// CI fixture transform, never used by the desktop installer or catalog validator.
// Preserve compiled example code; vary only its reviewed manifest for update tests.
import assert from "node:assert/strict";
import { gunzipSync, gzipSync } from "node:zlib";
export function reversionFixture(bytes, version, extraPermission) {
  return transformFixture(bytes, (name, data) => {
    if (name !== "manifest.json") return data;
    const manifest = JSON.parse(data);
    assert.equal(manifest.id, "codemux.project-brief");
    manifest.version = version;
    if (extraPermission) manifest.permissions.push(extraPermission);
    return Buffer.from(JSON.stringify(manifest, null, 2));
  });
}
export function transformFixture(bytes, transform) {
  const tar = gunzipSync(bytes);
  const files = [];
  for (let offset = 0; offset < tar.length && tar[offset]; ) {
    const header = Buffer.from(tar.subarray(offset, offset + 512));
    const name = header.toString("utf8", 0, 100).split("\0")[0];
    const size = parseInt(
      header.toString("ascii", 124, 136).replace(/\0.*$/, "").trim(),
      8,
    );
    assert.ok(Number.isSafeInteger(size) && size >= 0);
    assert.equal(
      header[156],
      48,
      "Only regular author-package fixture entries are accepted",
    );
    let data = tar.subarray(offset + 512, offset + 512 + size);
    offset += 512 + Math.ceil(size / 512) * 512;
    const replacement = transform(name, data);
    if (replacement !== data) {
      data = replacement;
      header.write(data.length.toString(8).padStart(11, "0") + "\0", 124);
      header.fill(32, 148, 156);
      header.write(
        [...header]
          .reduce((a, b) => a + b, 0)
          .toString(8)
          .padStart(6, "0") + "\0 ",
        148,
      );
    }
    files.push(header, data, Buffer.alloc((512 - (data.length % 512)) % 512));
  }
  return gzipSync(Buffer.concat([...files, Buffer.alloc(1024)]), { level: 9 });
}
