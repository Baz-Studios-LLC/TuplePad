// A real screen reader reads TuplePad: Guidepup starts VoiceOver (macOS) or NVDA (Windows),
// TuplePad opens a note, the arrow keys move the caret, and what the screen reader said is checked.
//
//   npx @guidepup/setup && node .github/screen-reader.mjs <tuplepad-app>
import { nvda, voiceOver } from "@guidepup/guidepup";
import { spawn } from "node:child_process";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const sleep = (ms) => new Promise((done) => setTimeout(done, ms));
const app = resolve(process.argv[2]);
const reader = process.platform === "darwin" ? voiceOver : nvda;
const name = process.platform === "darwin" ? "VoiceOver" : "NVDA";

const work = mkdtempSync(join(tmpdir(), "tuplepad-reader-"));
const note = join(work, "notes.md");
writeFileSync(note, "# Notes\n\nSecond line here\n");

await reader.start();
// The real app (not a scripted run), with a data folder of its own.
const tuplepad = spawn(app, [note], { env: { ...process.env, TUPLEPAD_DATA_DIR: join(work, "data") }, stdio: "ignore" });
let spoken = [];
try {
  await sleep(10000);
  // Down twice: the empty line, then "Second line here".
  await reader.press("ArrowDown");
  await sleep(3000);
  await reader.press("ArrowDown");
  await sleep(3000);
  // Then "where am I?": each screen reader's command to describe what has the keyboard focus.
  if (reader === nvda) {
    await nvda.perform(nvda.keyboardCommands.reportCurrentFocus);
  } else {
    await voiceOver.perform(voiceOver.keyboardCommands.describeItemWithKeyboardFocus);
  }
  await sleep(5000);
  spoken = await reader.spokenPhraseLog();
} finally {
  tuplepad.kill();
  await reader.stop();
}

console.log(`What ${name} said:`);
for (const phrase of spoken) console.log(`  ${phrase}`);
const said = spoken.join(" | ");
// (NVDA says "notes dot md".)
const missing = ["Editor, notes", "Second line here"].filter((expected) => !said.includes(expected));
if (missing.length > 0) {
  console.error(`${name} never said: ${missing.join(", ")}`);
  process.exit(1);
}
console.log(`${name} names the document and reads the line the caret moves to.`);
