// Actual Windows WebView2 workflow checks. The debug-only CDP endpoint is bound
// by the platform runtime; production launches never enable it in application code.
import { chromium } from "@playwright/test";
import { spawn, execFileSync } from "node:child_process";
import { mkdir, mkdtemp, writeFile, readFile } from "node:fs/promises";
import { resolve, join } from "node:path";
import { tmpdir } from "node:os";
import assert from "node:assert/strict";
const binary = resolve(process.argv[2] ?? "target/debug/gitcrab.exe");
const output = resolve(process.argv[3] ?? "dist/native-e2e");
await mkdir(output, { recursive: true });
const fixture = await mkdtemp(join(tmpdir(), "gitcrab-tauri-e2e-"));
const git = (...args) =>
  execFileSync("git", ["-C", fixture, ...args], {
    encoding: "utf8",
    windowsHide: true,
  }).trim();
git("init", "--quiet", "--initial-branch=main");
git("config", "user.name", "GitCrab E2E");
git("config", "user.email", "e2e@example.invalid");
git("config", "commit.gpgsign", "false");
git("config", "core.autocrlf", "false");
await mkdir(join(fixture, "hooks"));
git("config", "core.hooksPath", join(fixture, "hooks"));
const original =
  Array.from({ length: 60 }, (_, i) => `// Application line ${i}`).join("\n") +
  "\n";
await writeFile(join(fixture, "app.rs"), original);
git("add", "--", "app.rs");
git("commit", "--quiet", "-m", "Initialize application");
git("switch", "--quiet", "-c", "feature/graph");
await writeFile(join(fixture, "graph.txt"), "Independent graph\n");
git("add", ".");
git("commit", "--quiet", "-m", "Implement graph");
git("switch", "--quiet", "main");
await writeFile(join(fixture, "main.txt"), "Main branch\n");
git("add", ".");
git("commit", "--quiet", "-m", "Develop main branch");
git("merge", "--quiet", "--no-ff", "feature/graph", "-m", "Merge graph");
git("tag", "v0.1");
await writeFile(
  join(fixture, "app.rs"),
  original.replace("line 5", "staged line 5"),
);
git("add", "app.rs");
await writeFile(
  join(fixture, "app.rs"),
  original
    .replace("line 5", "staged line 5")
    .replace("line 45", "unstaged line 45"),
);
await writeFile(
  join(fixture, "notes untracked ü.txt"),
  "Untracked Unicode note\n",
);
const child = spawn(binary, [fixture], {
  windowsHide: true,
  env: {
    ...process.env,
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:
      "--remote-debugging-port=9223 --remote-debugging-address=127.0.0.1",
    WEBVIEW2_USER_DATA_FOLDER: join(output, "webview-profile"),
  },
});
let browser;
let processError = "";
child.stderr.on("data", (chunk) => {
  processError += chunk.toString();
});
child.on("error", (error) => {
  processError += error.message;
});
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
try {
  for (let i = 0; i < 240; i++) {
    if (child.exitCode !== null)
      throw Error(`Native app exited ${child.exitCode}: ${processError}`);
    try {
      browser = await chromium.connectOverCDP("http://127.0.0.1:9223");
      break;
    } catch {
      await sleep(250);
    }
  }
  assert(
    browser,
    `Native WebView2 CDP did not start (exit=${child.exitCode}, signal=${child.signalCode}): ${processError}`,
  );
  const pages = browser.contexts().flatMap((c) => c.pages());
  const page = pages.find((p) => /tauri|localhost/.test(p.url())) ?? pages[0];
  assert(page, "App page exists");
  page.on("pageerror", (error) => {
    throw error;
  });
  await page.getByRole("option", { name: /Merge graph/ }).waitFor();
  await page.getByRole("button", { name: "◇ v0.1", exact: true }).waitFor();
  const capture = async (name) => {
    await page.screenshot({ path: join(output, `${name}.png`) });
  };
  await capture("history");
  await page.getByRole("option", { name: /Develop main branch/ }).click();
  await page.getByRole("button", { name: "▤ main.txt" }).waitFor();
  await capture("details");
  await page.getByRole("button", { name: "▤ main.txt" }).click();
  await page.getByRole("region", { name: "Diff for main.txt" }).waitFor();
  await capture("commitdiff");
  await page.getByRole("button", { name: "Close diff" }).click();
  await page.getByRole("option", { name: /WIP/ }).click();
  await capture("staging");
  await page
    .getByRole("textbox", { name: "Commit message" })
    .fill("Preserve this draft");
  await page
    .locator(".file-section")
    .first()
    .getByRole("button", { name: "app.rs", exact: true })
    .click();
  await page.getByRole("region", { name: "Diff for app.rs" }).waitFor();
  await capture("diff");
  await page.getByRole("checkbox", { name: "Split view", exact: true }).check();
  await capture("split");
  await page
    .getByRole("checkbox", { name: "Split view", exact: true })
    .uncheck();
  await page.getByRole("button", { name: "Stage hunk", exact: true }).click();
  await page
    .getByRole("status")
    .filter({ hasText: "Index hunk updated" })
    .waitFor();
  assert(git("diff", "--cached", "--", "app.rs").includes("unstaged line 45"));
  assert.equal(
    await readFile(join(fixture, "app.rs"), "utf8"),
    original
      .replace("line 5", "staged line 5")
      .replace("line 45", "unstaged line 45"),
  );
  await page.getByRole("button", { name: "Staged", exact: true }).click();
  await page
    .getByRole("button", { name: "Unstage hunk", exact: true })
    .first()
    .waitFor();
  await page
    .getByRole("button", { name: "Unstage hunk", exact: true })
    .first()
    .click();
  await page
    .getByRole("status")
    .filter({ hasText: "Index hunk updated" })
    .waitFor();
  assert.equal(
    await page.getByRole("textbox", { name: "Commit message" }).inputValue(),
    "Preserve this draft",
  );
  // Branch operation errors refresh status and preserve the editor.
  await page
    .getByRole("button", { name: /Checkout/ })
    .first()
    .click();
  await page
    .getByRole("textbox", { name: "Operation target" })
    .fill("missing-e2e-branch");
  await page.getByRole("button", { name: "Run Checkout" }).click();
  await page.getByRole("alert").waitFor();
  assert.equal(
    await page.getByRole("textbox", { name: "Commit message" }).inputValue(),
    "Preserve this draft",
  );
  // A failing hook must preserve the draft; a later successful commit clears it.
  await writeFile(
    join(fixture, "hooks", "pre-commit"),
    '#!/bin/sh\necho "Expected hook rejection" >&2\nexit 1\n',
  );
  await page
    .getByRole("button", { name: "Commit to main", exact: true })
    .click();
  await page
    .getByRole("alert")
    .filter({ hasText: "Expected hook rejection" })
    .waitFor();
  assert.equal(
    await page.getByRole("textbox", { name: "Commit message" }).inputValue(),
    "Preserve this draft",
  );
  await writeFile(join(fixture, "hooks", "pre-commit"), "#!/bin/sh\nexit 0\n");
  await page
    .getByRole("button", { name: "Commit to main", exact: true })
    .click();
  await page
    .getByRole("status")
    .filter({ hasText: "Commit created" })
    .waitFor();
  assert.equal(
    await page.getByRole("textbox", { name: "Commit message" }).inputValue(),
    "",
  );
  assert.equal(git("log", "-1", "--format=%s"), "Preserve this draft");
  // Arrow navigation, context review and narrow layout.
  if (await page.getByRole("button", { name: "Close diff" }).count())
    await page.getByRole("button", { name: "Close diff" }).click();
  await page.getByRole("listbox").focus();
  await page.keyboard.press("ArrowDown");
  assert((await page.getByRole("option", { selected: true }).count()) === 1);
  await page.getByRole("option", { selected: true }).click({ button: "right" });
  await page.getByRole("menuitem", { name: /Cherry-pick/ }).waitFor();
  await page.keyboard.press("Escape");
  await page.getByRole("listbox").focus();
  await page.keyboard.press("Shift+F10");
  await page.getByRole("menuitem", { name: /Cherry-pick/ }).waitFor();
  await page.keyboard.press("Escape");
  const viewport = await page.evaluate(() => ({
    width: innerWidth,
    height: innerHeight,
    dpr: devicePixelRatio,
  }));
  await writeFile(
    join(output, "result.json"),
    JSON.stringify(
      {
        fixture,
        binary,
        viewport,
        checks:
          "history/details/staging/unified/split/committed diff; index-only hunk stage/reverse; draft preservation after Git/hook failures; commit success; arrow navigation; context menu",
        passed: true,
      },
      null,
      2,
    ),
  );
  console.log("Native WebView workflows passed", JSON.stringify(viewport));
} finally {
  await browser?.close();
  child.kill();
}
