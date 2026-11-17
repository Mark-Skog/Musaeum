// The system clipboard, for tests that check what the app copies: what is on it is put aside when
// the test starts and put back when it ends, so a person's clipboard is as they left it.
//
// Windows Forms through a hidden PowerShell in a single-threaded apartment, which the clipboard
// needs. Text, a picture or a list of files are kept; anything else (a spreadsheet's cells, say)
// is kept as the text it also offers, if it offers one.

import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

function powershell(script) {
  const prelude =
    'Add-Type -AssemblyName System.Windows.Forms; Add-Type -AssemblyName System.Drawing; ' +
    '$c = [System.Windows.Forms.Clipboard]; $utf8 = New-Object System.Text.UTF8Encoding($false); ';
  return execFileSync('powershell', ['-NoProfile', '-NonInteractive', '-STA', '-Command', prelude + script], {
    encoding: 'utf8',
    windowsHide: true,
  }).trim();
}

const quoted = (path) => `'${path.replace(/'/g, "''")}'`;

/** Puts what is on the clipboard aside in `folder`, and returns how to put it back. */
export function saveClipboard(folder) {
  const base = join(folder, 'clipboard-kept');
  const kind = powershell(
    `if ($c::ContainsFileDropList()) { [IO.File]::WriteAllLines(${quoted(`${base}.files`)}, [string[]]@($c::GetFileDropList()), $utf8); 'files' } ` +
      `elseif ($c::ContainsImage()) { $i = $c::GetImage(); $i.Save(${quoted(`${base}.png`)}, [System.Drawing.Imaging.ImageFormat]::Png); $i.Dispose(); 'image' } ` +
      `elseif ($c::ContainsText()) { [IO.File]::WriteAllText(${quoted(`${base}.txt`)}, $c::GetText(), $utf8); 'text' } ` +
      `else { 'empty' }`,
  );
  return () => {
    if (kind === 'files' && existsSync(`${base}.files`)) {
      powershell(
        `$l = New-Object System.Collections.Specialized.StringCollection; ` +
          `$l.AddRange([IO.File]::ReadAllLines(${quoted(`${base}.files`)}, $utf8)); $c::SetFileDropList($l)`,
      );
    } else if (kind === 'image' && existsSync(`${base}.png`)) {
      powershell(`$i = [System.Drawing.Image]::FromFile(${quoted(`${base}.png`)}); $c::SetImage($i); $i.Dispose()`);
    } else if (kind === 'text' && existsSync(`${base}.txt`)) {
      powershell(`$c::SetText([IO.File]::ReadAllText(${quoted(`${base}.txt`)}, $utf8))`);
    } else {
      powershell('$c::Clear()');
    }
    return kind;
  };
}

/** Puts `text` on the clipboard. */
export function setClipboardText(text, folder) {
  const path = join(folder, 'clipboard-set.txt');
  writeFileSync(path, text, 'utf8');
  powershell(`$c::SetText([IO.File]::ReadAllText(${quoted(path)}, $utf8))`);
}

/** The text on the clipboard, or null if it has none. */
export function clipboardText(folder) {
  const path = join(folder, 'clipboard-read.txt');
  const has = powershell(`if ($c::ContainsText()) { [IO.File]::WriteAllText(${quoted(path)}, $c::GetText(), $utf8); 'yes' } else { 'no' }`);
  return has === 'yes' ? readFileSync(path, 'utf8') : null;
}
