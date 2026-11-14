import { version } from "../../package.json";

export const PRODUCT_NAME = "Nickel";
export const PRODUCT_SHORT_NAME = "Ni";
export const PRODUCT_VERSION = version;
export const PROJECT_URL = "https://github.com/Nickel-Studio/Ni-Launcher";
export const CODE_LICENSE = "AGPL-3.0-only";
export const ASSETS_LICENSE = "CC-BY-SA-4.0";
export const CODE_LICENSE_URL = `${PROJECT_URL}/blob/main/LICENSE`;
export const ASSETS_LICENSE_URL = `${PROJECT_URL}/blob/main/LICENSE-ASSETS`;

export interface BrandCredit {
  id: string;
  name: string;
  url?: string;
  linkLabel?: string;
}

export const DEVELOPERS: readonly BrandCredit[] = [
  {
    id: "itzarona",
    name: "ItzArona",
    url: "https://github.com/ItzArona",
    linkLabel: "GitHub 主页",
  },
  {
    id: "itzplana",
    name: "ItzPlana",
    url: "https://github.com/ItzPlana",
    linkLabel: "GitHub 主页",
  },
];

export const SPECIAL_THANKS: readonly BrandCredit[] = [
  { id: "longmao", name: "龙猫腾跃" },
  {
    id: "pcl",
    name: "PCL 启动器",
    url: "https://github.com/Meloong-Git/PCL",
    linkLabel: "项目仓库",
  },
  {
    id: "pcl-ce",
    name: "PCL-CE 启动器",
    url: "https://github.com/PCL-Community/PCL-CE",
    linkLabel: "项目仓库",
  },
  {
    id: "linuxdo",
    name: "LinuxDO 社区",
    url: "https://linux.do",
    linkLabel: "社区主页",
  },
  {
    id: "shittimchests",
    name: "ShittimChests 组织",
    url: "https://github.com/ShittimChests",
    linkLabel: "GitHub 主页",
  },
];
