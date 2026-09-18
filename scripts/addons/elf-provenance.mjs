// linuxdeploy adjusts ELF RPATH while assembling AppImages. Compare every
// section except those exact metadata tables, then compare their semantics.
import assert from "node:assert/strict";
function elf(bytes) {
  assert.equal(
    bytes.subarray(0, 6).toString("hex"),
    "7f454c460201",
    "Expected little-endian ELF64",
  );
  const number = (offset) => {
    const value = bytes.readBigUInt64LE(offset);
    assert.ok(value <= BigInt(Number.MAX_SAFE_INTEGER));
    return Number(value);
  };
  const slice = (offset, size) => {
    assert.ok(
      offset >= 0 && size >= 0 && offset + size <= bytes.length,
      "Invalid ELF section bounds",
    );
    return bytes.subarray(offset, offset + size);
  };
  const table = number(40),
    count = bytes.readUInt16LE(60),
    stride = bytes.readUInt16LE(58);
  assert.ok(count > 0 && stride >= 64);
  slice(table, count * stride);
  const raw = Array.from({ length: count }, (_, index) => {
    const at = table + index * stride;
    return {
      nameOffset: bytes.readUInt32LE(at),
      type: bytes.readUInt32LE(at + 4),
      flags: number(at + 8),
      address: number(at + 16),
      offset: number(at + 24),
      size: number(at + 32),
      link: bytes.readUInt32LE(at + 40),
    };
  });
  const names = raw[bytes.readUInt16LE(62)];
  assert.ok(names);
  const string = (data, at) => {
    assert.ok(at < data.length);
    const end = data.indexOf(0, at);
    assert.ok(end >= at);
    return data.subarray(at, end).toString("utf8");
  };
  const strings = slice(names.offset, names.size);
  const sections = new Map(
    raw
      .slice(1)
      .map((section) => [
        string(strings, section.nameOffset),
        {
          ...section,
          data:
            section.type === 8
              ? Buffer.alloc(0)
              : slice(section.offset, section.size),
        },
      ]),
  );
  const symbols = (section) => {
    const table = raw[section.link];
    const names = slice(table.offset, table.size);
    const result = [];
    assert.equal(section.size % 24, 0);
    for (let at = 0; at < section.size; at += 24) {
      const data = section.data;
      const index = data.readUInt16LE(at + 6);
      const location = Number(data.readBigUInt64LE(at + 8));
      const target = index > 0 && index < 0xff00 ? raw[index] : undefined;
      result.push([
        string(names, data.readUInt32LE(at)),
        data[at + 4],
        data[at + 5],
        target ? string(strings, target.nameOffset) : index,
        section.type === 2
          ? location
          : target
            ? location - target.address
            : location,
        data.readBigUInt64LE(at + 16).toString(),
      ]);
    }
    return result;
  };
  const dynamic = sections.get(".dynamic"),
    dynstr = sections.get(".dynstr");
  assert.ok(dynamic && dynstr);
  const entries = [],
    paths = [];
  // Dynamic entries whose values point at sections rather than being lengths,
  // flags or string offsets. Address relocation is normalized to section+offset.
  const pointers = new Set([
    3, 4, 5, 6, 7, 12, 13, 17, 21, 23, 25, 26, 32, 0x6ffffef5, 0x6ffffff0,
    0x6ffffffc, 0x6ffffffe,
  ]);
  for (let at = 0; at < dynamic.data.length; at += 16) {
    const tag = Number(dynamic.data.readBigInt64LE(at));
    const value = Number(dynamic.data.readBigUInt64LE(at + 8));
    if (tag === 0) break;
    if (tag === 15 || tag === 29) {
      paths.push(string(dynstr.data, value));
      continue;
    }
    if (tag === 10) continue; // DT_STRSZ grows only with the rewritten path.
    let semantic = value;
    if ([1, 14].includes(tag)) semantic = string(dynstr.data, value);
    else if (pointers.has(tag) && value !== 0) {
      const section = [...sections].find(
        ([, s]) => value >= s.address && value < s.address + s.size,
      );
      assert.ok(section, `Unknown dynamic pointer ${tag}`);
      semantic = `${section[0]}+${value - section[1].address}`;
    }
    entries.push([tag, semantic]);
  }
  entries.sort((a, b) => JSON.stringify(a).localeCompare(JSON.stringify(b)));
  return {
    type: bytes.readUInt16LE(16),
    machine: bytes.readUInt16LE(18),
    entry: number(24),
    sections,
    entries,
    paths,
    symbols,
  };
}
export function verifyAppImageElf(original, packaged) {
  const before = elf(original),
    after = elf(packaged);
  assert.equal(after.type, before.type);
  assert.equal(after.machine, before.machine);
  assert.equal(after.entry, before.entry);
  assert.deepEqual(
    [...after.sections.keys()].sort(),
    [...before.sections.keys()].sort(),
  );
  for (const [name, section] of before.sections) {
    const candidate = after.sections.get(name);
    assert.equal(candidate.type, section.type, `${name}: section type changed`);
    assert.equal(
      candidate.flags,
      section.flags,
      `${name}: section permissions changed`,
    );
    if ([".dynamic", ".dynstr", ".shstrtab"].includes(name)) continue;
    assert.equal(candidate.size, section.size, `${name}: section size changed`);
    if (section.type === 2 || section.type === 11) {
      assert.deepEqual(
        after.symbols(candidate),
        before.symbols(section),
        `${name}: symbol semantics changed`,
      );
      continue;
    }
    assert.ok(
      candidate.data.equals(section.data),
      `${name}: program section differs from release build`,
    );
  }
  assert.deepEqual(
    after.entries,
    before.entries,
    "Dynamic-link metadata changed beyond RPATH relocation",
  );
  assert.deepEqual(
    after.paths,
    ["$ORIGIN"],
    "Unexpected AppImage host runtime search path",
  );
  return "elf-sections-and-dynamic-metadata";
}
