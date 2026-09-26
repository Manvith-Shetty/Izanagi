// Screenshot a page into `temporary screenshots/`, next to this file.
//
//   node tab/web/screenshot.mjs http://localhost:5173          -> screenshot-N.png
//   node tab/web/screenshot.mjs http://localhost:5173 hero     -> screenshot-N-hero.png
//
// Works from any working directory. N auto-increments; nothing is ever overwritten.
import puppeteer from "puppeteer";
import { mkdir, readdir } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const [url, label] = process.argv.slice(2);
if (!url || !/^https?:\/\/localhost[:/]/.test(url)) {
  console.error("usage: node screenshot.mjs http://localhost:PORT [label]  (localhost only, never file://)");
  process.exit(1);
}

const dir = join(dirname(fileURLToPath(import.meta.url)), "temporary screenshots");
await mkdir(dir, { recursive: true });

const taken = (await readdir(dir))
  .map((f) => f.match(/^screenshot-(\d+)/)?.[1])
  .filter(Boolean)
  .map(Number);
const n = taken.length ? Math.max(...taken) + 1 : 1;
const file = join(dir, `screenshot-${n}${label ? `-${label}` : ""}.png`);

const browser = await puppeteer.launch();
try {
  const page = await browser.newPage();
  await page.setViewport({ width: 1440, height: 900, deviceScaleFactor: 1 });
  await page.goto(url, { waitUntil: "networkidle0", timeout: 30_000 });
  await page.screenshot({ path: file, fullPage: true });
} finally {
  await browser.close();
}
console.log(file);
