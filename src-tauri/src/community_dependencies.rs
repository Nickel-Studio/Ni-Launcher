//! Required dependency metadata and exact project/version lookup. No downloads.
use crate::community_network::{self, Platform};
use serde::Serialize;
use serde_json::{Value, json};
use tauri::AppHandle;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RequiredDependency {
    dependency_type: &'static str,
    project_id: String,
    version_id: String,
    file_name: String,
}

fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or("").trim().to_owned()
}

pub(crate) fn modrinth_dependencies(value: &Value) -> Option<Vec<RequiredDependency>> {
    value["dependencies"].as_array().map(|dependencies| {
        dependencies
            .iter()
            .filter(|dependency| dependency["dependency_type"].as_str() == Some("required"))
            .map(|dependency| RequiredDependency {
                dependency_type: "required",
                project_id: text(dependency, "project_id"),
                version_id: text(dependency, "version_id"),
                file_name: text(dependency, "file_name"),
            })
            .collect()
    })
}

pub(crate) fn curseforge_dependencies(value: &Value) -> Option<Vec<RequiredDependency>> {
    value["dependencies"].as_array().map(|dependencies| {
        dependencies
            .iter()
            .filter(|dependency| dependency["relationType"].as_u64() == Some(3))
            .map(|dependency| RequiredDependency {
                dependency_type: "required",
                project_id: dependency["modId"]
                    .as_u64()
                    .filter(|id| *id > 0)
                    .map(|id| id.to_string())
                    .unwrap_or_default(),
                version_id: String::new(),
                file_name: String::new(),
            })
            .collect()
    })
}

fn modrinth_id(value: &str) -> Result<&str, String> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err("Modrinth 项目或版本标识无效".into());
    }
    Ok(value)
}

fn curseforge_id(value: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| "CurseForge 项目或文件编号必须为正整数".into())
}

async fn metadata(
    platform: Platform,
    suffix: &str,
    settings: &crate::network_settings::NetworkSettings,
    transfer: &minecraft_java_rs_core::net::transfer::TransferContext,
    key: Option<&str>,
) -> Result<Value, String> {
    community_network::get_json(
        platform,
        &format!("{}{suffix}", platform.api()),
        settings,
        transfer,
        key,
        |value| {
            let data = if platform == Platform::Curseforge {
                &value["data"]
            } else {
                value
            };
            if !data.is_object() {
                return Err("前置元数据结构无效，请重试".into());
            }
            Ok(data.clone())
        },
    )
    .await
}

fn normalized_project(
    platform: Platform,
    project: &Value,
    versions: &Value,
) -> Result<Value, String> {
    let output = match platform {
        Platform::Modrinth => {
            let mut hit = project.clone();
            hit["project_id"] = project["id"].clone();
            hit["title"] = project["title"].clone();
            hit["date_modified"] = project["updated"].clone();
            if let Some(items) = versions.as_array() {
                let mut loaders = std::collections::BTreeSet::new();
                let mut game_versions = std::collections::BTreeSet::new();
                for version in items {
                    for loader in version["loaders"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                    {
                        loaders.insert(loader);
                    }
                    for game_version in version["gameVersions"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                    {
                        game_versions.insert(game_version);
                    }
                }
                hit["loaders"] = json!(loaders);
                hit["versions"] = json!(game_versions);
            }
            crate::modrinth::normalize_search(&json!({ "hits": [hit] }))?
        }
        Platform::Curseforge => crate::curseforge::normalize_search(&json!({ "data": [project] }))?,
    };
    let normalized: Value = serde_json::from_str(&output).map_err(|_| "前置项目信息格式无效")?;
    let project = &normalized["hits"][0];
    if !project.is_object() || project["projectId"].as_str().unwrap_or("").is_empty() {
        return Err("前置项目已删除或缺少有效标识，请重试".into());
    }
    Ok(project.clone())
}

fn modrinth_version_project(version: &Value, requested_version: &str) -> Result<String, String> {
    if text(version, "id") != requested_version {
        return Err("返回的前置版本与声明不一致，请重试".into());
    }
    let project_id = text(version, "project_id");
    modrinth_id(&project_id)?;
    Ok(project_id)
}

fn validate_project_identity(
    platform: Platform,
    project: &Value,
    lookup_project: &str,
    requested_project: &str,
    version_pinned: bool,
) -> Result<(), String> {
    let actual_id = match platform {
        Platform::Modrinth => {
            let id = text(project, "id");
            modrinth_id(&id)?;
            id
        }
        Platform::Curseforge => project["id"]
            .as_u64()
            .filter(|id| *id > 0)
            .map(|id| id.to_string())
            .ok_or("前置项目缺少有效编号，请重试")?,
    };
    let lookup_matches = actual_id == lookup_project
        || (platform == Platform::Modrinth
            && !version_pinned
            && text(project, "slug") == lookup_project);
    if !lookup_matches {
        return Err("返回的前置项目与请求不一致，请重试".into());
    }
    if !requested_project.is_empty() {
        let requested_matches = match platform {
            Platform::Modrinth => {
                requested_project == actual_id || requested_project == text(project, "slug")
            }
            Platform::Curseforge => curseforge_id(requested_project)?.to_string() == actual_id,
        };
        if !requested_matches {
            return Err("指定前置版本不属于所声明项目，请重试".into());
        }
    }
    Ok(())
}

/// Resolve only identifiers supplied by metadata; never select a latest release.
#[tauri::command]
pub async fn community_dependency_resolve(
    app: AppHandle,
    platform: String,
    project_id: Option<String>,
    version_id: Option<String>,
) -> Result<String, String> {
    let platform = match platform.as_str() {
        "modrinth" => Platform::Modrinth,
        "curseforge" => Platform::Curseforge,
        _ => return Err("前置资源来源无效".into()),
    };
    let requested_project = project_id.as_deref().unwrap_or("").trim();
    let requested_version = version_id.as_deref().unwrap_or("").trim();
    if requested_project.is_empty() && requested_version.is_empty() {
        return Err("该前置仅提供文件名，无法定位社区项目".into());
    }
    let settings = crate::network_settings::snapshot();
    let transfer = community_network::context(&settings);
    let key = if platform == Platform::Curseforge {
        crate::secrets::curseforge_api_key(app)?
    } else {
        None
    };
    let mut normalized_versions = json!([]);
    let project_id = match platform {
        Platform::Modrinth => {
            if !requested_version.is_empty() {
                let version_id = modrinth_id(requested_version)?;
                let version = metadata(
                    platform,
                    &format!("version/{version_id}"),
                    &settings,
                    &transfer,
                    None,
                )
                .await?;
                let actual_project = modrinth_version_project(&version, requested_version)?;
                let text = crate::modrinth::normalize_dependency_versions(&json!([version]))?;
                normalized_versions =
                    serde_json::from_str(&text).map_err(|_| "前置版本格式无效")?;
                actual_project
            } else {
                modrinth_id(requested_project)?.to_owned()
            }
        }
        Platform::Curseforge => {
            let project_id = curseforge_id(requested_project)?;
            if !requested_version.is_empty() {
                let file_id = curseforge_id(requested_version)?;
                let file = metadata(
                    platform,
                    &format!("mods/{project_id}/files/{file_id}"),
                    &settings,
                    &transfer,
                    key.as_deref(),
                )
                .await?;
                if file["id"].as_u64() != Some(file_id)
                    || file["modId"].as_u64() != Some(project_id)
                {
                    return Err("前置文件不属于所声明项目，请重试".into());
                }
                normalized_versions = crate::curseforge::normalize_dependency_versions(&[file])?;
            }
            project_id.to_string()
        }
    };
    let suffix = match platform {
        Platform::Modrinth => format!("project/{project_id}"),
        Platform::Curseforge => format!("mods/{project_id}"),
    };
    let project = metadata(platform, &suffix, &settings, &transfer, key.as_deref()).await?;
    validate_project_identity(
        platform,
        &project,
        &project_id,
        requested_project,
        !requested_version.is_empty(),
    )?;
    let project = normalized_project(platform, &project, &normalized_versions)?;
    let version = if requested_version.is_empty() {
        Value::Null
    } else {
        normalized_versions[0].clone()
    };
    if !requested_version.is_empty() && !version.is_object() {
        return Err("指定前置版本不可用，请重试".into());
    }
    serde_json::to_string(&json!({ "project": project, "version": version }))
        .map_err(|error| format!("前置元数据序列化失败：{error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_modrinth_metadata_keeps_all_supported_identifiers() {
        let value = json!({ "dependencies": [
            { "dependency_type": "required", "project_id": "fabric-api" },
            { "dependency_type": "required", "project_id": "quilted-fabric-api" },
            { "dependency_type": "required", "version_id": "PinnedVersion" },
            { "dependency_type": "required", "file_name": "external.jar" },
            { "dependency_type": "optional", "project_id": "optional" },
            { "dependency_type": "incompatible", "project_id": "incompatible" },
            { "dependency_type": "embedded", "project_id": "embedded" }
        ] });
        let dependencies = modrinth_dependencies(&value).unwrap();
        assert_eq!(dependencies.len(), 4);
        assert_eq!(dependencies[0].project_id, "fabric-api");
        assert_eq!(dependencies[1].project_id, "quilted-fabric-api");
        assert_eq!(dependencies[2].version_id, "PinnedVersion");
        assert_eq!(dependencies[3].file_name, "external.jar");
    }

    #[test]
    fn unavailable_metadata_is_not_a_known_empty_dependency_list() {
        for value in [
            json!({}),
            json!({ "dependencies": null }),
            json!({ "dependencies": {} }),
        ] {
            assert!(modrinth_dependencies(&value).is_none());
            assert!(curseforge_dependencies(&value).is_none());
        }
        assert_eq!(
            modrinth_dependencies(&json!({ "dependencies": [] })),
            Some(vec![])
        );
        assert_eq!(
            curseforge_dependencies(&json!({ "dependencies": [] })),
            Some(vec![])
        );
    }

    #[test]
    fn curseforge_only_numeric_relation_three_is_required() {
        let value = json!({ "dependencies": [
            { "relationType": 3, "modId": 306612 },
            { "relationType": 3, "modId": 634179 },
            { "relationType": 2, "modId": 2 },
            { "relationType": 4, "modId": 4 },
            { "relationType": 5, "modId": 5 },
            { "relationType": "3", "modId": 6 },
            { "relationType": 3.0, "modId": 7 }
        ] });
        let dependencies = curseforge_dependencies(&value).unwrap();
        assert_eq!(dependencies.len(), 2);
        assert_eq!(dependencies[0].project_id, "306612");
        assert_eq!(dependencies[1].project_id, "634179");
        assert!(
            dependencies
                .iter()
                .all(|dependency| dependency.version_id.is_empty())
        );
    }

    #[test]
    fn lookup_identifiers_cannot_escape_api_paths() {
        for id in [
            "",
            "../project",
            "a/b",
            "https://example.org",
            "x?query",
            "x#hash",
        ] {
            assert!(modrinth_id(id).is_err());
        }
        assert!(modrinth_id(&"a".repeat(129)).is_err());
        assert!(modrinth_id("fabric-api").is_ok());
        assert!(modrinth_id("PinnedVersion123").is_ok());
        assert!(curseforge_id("306612").is_ok());
        for id in ["0", "-1", "../1", "1.2", ""] {
            assert!(curseforge_id(id).is_err());
        }
    }

    #[test]
    fn project_only_modrinth_slug_accepts_its_canonical_identity() {
        let project = json!({ "id": "CanonicalAPI", "slug": "fabric-api" });
        assert!(
            validate_project_identity(
                Platform::Modrinth,
                &project,
                "fabric-api",
                "fabric-api",
                false
            )
            .is_ok()
        );
        assert!(
            validate_project_identity(
                Platform::Modrinth,
                &project,
                "CanonicalAPI",
                "CanonicalAPI",
                false
            )
            .is_ok()
        );
        assert!(
            validate_project_identity(
                Platform::Modrinth,
                &project,
                "unrelated",
                "unrelated",
                false
            )
            .is_err()
        );
    }

    #[test]
    fn pinned_modrinth_version_requires_the_canonical_project() {
        let project = json!({ "id": "CanonicalAPI", "slug": "fabric-api" });
        assert!(
            validate_project_identity(
                Platform::Modrinth,
                &project,
                "CanonicalAPI",
                "fabric-api",
                true
            )
            .is_ok()
        );
        assert!(
            validate_project_identity(Platform::Modrinth, &project, "CanonicalAPI", "", true)
                .is_ok()
        );
        assert!(
            validate_project_identity(
                Platform::Modrinth,
                &project,
                "fabric-api",
                "fabric-api",
                true
            )
            .is_err()
        );
        assert!(
            validate_project_identity(
                Platform::Modrinth,
                &project,
                "CanonicalAPI",
                "wrong-project",
                true
            )
            .is_err()
        );
    }

    #[test]
    fn version_only_modrinth_keeps_the_exact_version_identity() {
        let version = json!({ "id": "ExactVersion", "project_id": "CanonicalAPI" });
        assert_eq!(
            modrinth_version_project(&version, "ExactVersion").unwrap(),
            "CanonicalAPI"
        );
        assert!(modrinth_version_project(&version, "NewerVersion").is_err());
        assert!(
            modrinth_version_project(&json!({ "id": "ExactVersion" }), "ExactVersion").is_err()
        );
        assert!(
            modrinth_version_project(
                &json!({ "id": "ExactVersion", "project_id": "../wrong" }),
                "ExactVersion"
            )
            .is_err()
        );
    }

    #[test]
    fn curseforge_project_identity_is_numeric_never_a_slug() {
        let project = json!({ "id": 306612, "slug": "fabric-api" });
        assert!(
            validate_project_identity(Platform::Curseforge, &project, "306612", "306612", false)
                .is_ok()
        );
        assert!(
            validate_project_identity(Platform::Curseforge, &project, "306612", "0306612", true)
                .is_ok()
        );
        assert!(
            validate_project_identity(
                Platform::Curseforge,
                &project,
                "fabric-api",
                "fabric-api",
                false
            )
            .is_err()
        );
        assert!(
            validate_project_identity(Platform::Curseforge, &project, "306612", "634179", true)
                .is_err()
        );
        assert!(
            validate_project_identity(Platform::Curseforge, &json!({ "id": 0 }), "0", "0", false)
                .is_err()
        );
    }

    #[test]
    fn dependency_normalization_retains_modrinth_version_boundaries() {
        let raw = json!([
            { "id": "ParentA", "project_id": "Parent", "dependencies": [
                { "dependency_type": "required", "version_id": "ExactA" },
                { "dependency_type": "optional", "project_id": "Optional" }
            ] },
            { "id": "ParentB", "project_id": "Parent", "dependencies": [
                { "dependency_type": "required", "project_id": "quilted-fabric-api" }
            ] }
        ]);
        let normalized: Value =
            serde_json::from_str(&crate::modrinth::normalize_dependency_versions(&raw).unwrap())
                .unwrap();
        assert_eq!(normalized[0]["id"], "ParentA");
        assert_eq!(normalized[0]["dependencies"].as_array().unwrap().len(), 1);
        assert_eq!(normalized[0]["dependencies"][0]["versionId"], "ExactA");
        assert_eq!(
            normalized[1]["dependencies"][0]["projectId"],
            "quilted-fabric-api"
        );
    }

    #[test]
    fn dependency_normalization_retains_curseforge_file_boundaries() {
        let files = [
            json!({ "id": 1001, "modId": 100, "gameVersions": ["1.20.1", "Fabric"], "dependencies": [
                { "relationType": 3, "modId": 306612 },
                { "relationType": 2, "modId": 999 }
            ] }),
            json!({ "id": 1002, "modId": 100, "gameVersions": ["1.21.1", "Quilt"], "dependencies": [
                { "relationType": 3, "modId": 634179 }
            ] }),
        ];
        let normalized = crate::curseforge::normalize_dependency_versions(&files).unwrap();
        assert_eq!(normalized[0]["id"], "1001");
        assert_eq!(normalized[0]["dependencies"].as_array().unwrap().len(), 1);
        assert_eq!(normalized[0]["dependencies"][0]["projectId"], "306612");
        assert_eq!(normalized[1]["dependencies"][0]["projectId"], "634179");
        assert_eq!(normalized[0]["loaders"], json!(["fabric"]));
        assert_eq!(normalized[1]["loaders"], json!(["quilt"]));
    }

    #[test]
    fn normalized_dependency_projects_use_stable_ids_and_exact_version_tags() {
        let project = json!({ "id": "CanonicalAPI", "slug": "fabric-api", "title": "Shared title", "project_type": "mod" });
        let versions = json!([{ "id": "ExactVersion", "loaders": ["fabric", "quilt"], "gameVersions": ["1.20.1"] }]);
        let normalized = normalized_project(Platform::Modrinth, &project, &versions).unwrap();
        assert_eq!(normalized["projectId"], "CanonicalAPI");
        assert_eq!(normalized["slug"], "fabric-api");
        assert_eq!(normalized["loaders"], json!(["fabric", "quilt"]));
        assert_eq!(normalized["versions"], json!(["1.20.1"]));
        assert!(
            normalized_project(
                Platform::Modrinth,
                &json!({ "slug": "missing-id" }),
                &json!([])
            )
            .is_err()
        );
    }
}
