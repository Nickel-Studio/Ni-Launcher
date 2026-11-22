import { createCipheriv, randomBytes } from "node:crypto";

// Synthetic authenticated envelopes for script tests; never read real configuration.
export function envelope(values) {
  const output = [Buffer.from("NICKCFG1"), Buffer.from([values.length])];
  for (const [id, value] of values) {
    const key = randomBytes(32);
    const left = randomBytes(32);
    const right = Buffer.from(key.map((byte, index) => byte ^ left[index]));
    const nonce = randomBytes(12);
    const cipher = createCipheriv("aes-256-gcm", key, nonce);
    cipher.setAAD(Buffer.concat([Buffer.from("NICKCFG1"), Buffer.from([id])]));
    const ciphertext = Buffer.concat([cipher.update(value), cipher.final(), cipher.getAuthTag()]);
    const length = Buffer.alloc(4);
    length.writeUInt32LE(ciphertext.length);
    output.push(Buffer.from([id]), left, right, nonce, length, ciphertext);
    key.fill(0);
  }
  return Buffer.concat(output);
}
