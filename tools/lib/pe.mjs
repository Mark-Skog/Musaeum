// Reads what a Windows executable or DLL asks the loader for: the names of the DLLs it imports,
// at start and delay-loaded. For checking a bundle without Visual Studio's dumpbin.

import { readFileSync } from 'node:fs';

/** The DLL names `path` imports, lowercase, in the order the file lists them. */
export function importsOf(path) {
  const file = readFileSync(path);
  if (file.readUInt16LE(0) !== 0x5a4d) {
    throw new Error(`${path} is not an executable (no MZ)`);
  }
  const pe = file.readUInt32LE(0x3c);
  if (file.readUInt32LE(pe) !== 0x4550) {
    throw new Error(`${path} has no PE header`);
  }
  const coff = pe + 4;
  const sections = file.readUInt16LE(coff + 2);
  const optionalSize = file.readUInt16LE(coff + 16);
  const optional = coff + 20;
  const magic = file.readUInt16LE(optional);
  const directories = optional + (magic === 0x20b ? 112 : 96);
  const table = optional + optionalSize;
  const ranges = [];
  for (let n = 0; n < sections; n++) {
    const at = table + n * 40;
    ranges.push({
      address: file.readUInt32LE(at + 12),
      size: Math.max(file.readUInt32LE(at + 8), file.readUInt32LE(at + 16)),
      raw: file.readUInt32LE(at + 20),
    });
  }
  const offset = (rva) => {
    const section = ranges.find((range) => rva >= range.address && rva < range.address + range.size);
    if (section === undefined) {
      throw new Error(`${path}: address ${rva.toString(16)} is in no section`);
    }
    return rva - section.address + section.raw;
  };
  const text = (rva) => {
    const start = offset(rva);
    const end = file.indexOf(0, start);
    return file.toString('latin1', start, end).toLowerCase();
  };
  const names = [];
  // Directory 1: the import table, 20-byte descriptors, the name at +12.
  const imports = file.readUInt32LE(directories + 8);
  if (imports !== 0) {
    for (let at = offset(imports); file.readUInt32LE(at + 12) !== 0; at += 20) {
      names.push(text(file.readUInt32LE(at + 12)));
    }
  }
  // Directory 13: delay-load imports, 32-byte descriptors, the name at +4.
  const delayed = file.readUInt32LE(directories + 13 * 8);
  if (delayed !== 0) {
    for (let at = offset(delayed); file.readUInt32LE(at + 4) !== 0; at += 32) {
      names.push(text(file.readUInt32LE(at + 4)));
    }
  }
  return names;
}

/** The Visual C++ runtime's DLLs, which a Windows without the Redistributable does not have. */
export const VC_RUNTIME = /^(vcruntime|msvcp|concrt|vccorlib|vcomp|mfc|ucrtbased)/i;
