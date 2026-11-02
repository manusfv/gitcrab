// Capture a real native WebView2, using existing read-only fixture states.
import { chromium } from "@playwright/test";
import { spawn } from "node:child_process";
import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { resolve, join } from "node:path";
import { tmpdir } from "node:os";
const [
  binary = "target/release/gitcrab.exe",
  repository = "fixtures/showcase",
  destination = "evidence/tauri",
] = process.argv.slice(2);
await mkdir(destination, { recursive: true });
const profile = await mkdtemp(join(tmpdir(), "gitcrab-capture-profile-"));
const child = spawn(resolve(binary), [resolve(repository)], {
  windowsHide: true,
  env: {
    ...process.env,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:
      "--remote-debugging-port=9224 --remote-debugging-address=127.0.0.1",
    WEBVIEW2_USER_DATA_FOLDER: profile,
  },
});
let browser;
try {
  for (let i = 0; i < 100; i++) {
    try {
      browser = await chromium.connectOverCDP("http://127.0.0.1:9224");
      break;
    } catch {
      await new Promise((r) => setTimeout(r, 200));
    }
  }
  if (!browser) throw Error("Native WebView2 did not start");
  const page = browser.contexts()[0].pages()[0];
  await page.bringToFront();
  await page
    .getByRole("option", { name: /Rename the archived component/ })
    .waitFor();
  const capture = async (name) =>
    page.screenshot({ path: join(destination, `${name}.png`) });
  await capture("history");
  await page
    .getByRole("option", { name: /Rename the archived component/ })
    .click();
  await page.getByRole("button", { name: "▤ archive.txt" }).waitFor();
  await capture("details");
  await page.getByRole("button", { name: "▤ archive.txt" }).click();
  await page.getByRole("region", { name: "Diff for archive.txt" }).waitFor();
  await capture("commitdiff");
  await page.getByRole("button", { name: "Close diff" }).click();
  await page.getByRole("option", { name: /WIP/ }).click();
  await capture("staging");
  await page
    .locator(".file-section")
    .first()
    .getByRole("button", { name: "app.rs", exact: true })
    .click();
  await page.getByRole("region", { name: "Diff for app.rs" }).waitFor();
  await capture("diff");
  await page.getByRole("checkbox", { name: "Split view", exact: true }).check();
  await capture("split");
  await page.setViewportSize({ width: 820, height: 560 });
  await capture("narrow");
  await writeFile(
    join(destination, "capture.json"),
    JSON.stringify(
      {
        source: "Actual native Tauri WebView2, embedded local assets",
        binary: resolve(binary),
        fixture: resolve(repository),
        viewport: {
          width: 1172,
          height: 636,
          dpr: await page.evaluate(() => devicePixelRatio),
        },
        narrow: { width: 820, height: 560 },
        reference:
          "the reference app 1173x668 includes Windows title/menu chrome; these captures are the WebView client area. No pixel-equivalence claim.",
      },
      null,
      2,
    ),
  );
  console.log("Captured native showcase views");
} finally {
  await browser?.close();
  child.kill();
}
