// Test-only embedded WebDriver exercises WKWebView, WebKitGTK and WebView2.
// Production releases refuse to compile with the native-smoke feature.
import { spawn, execFileSync } from "node:child_process";
import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve, join } from "node:path";
const binary = resolve(
    process.argv[2] ??
      `target/debug/gitcrab${process.platform === "win32" ? ".exe" : ""}`,
  ),
  output = resolve(process.argv[3] ?? "dist/native-smoke");
await mkdir(output, { recursive: true });
const fixture = await mkdtemp(join(tmpdir(), "gitcrab-smoke-"));
const git = (...args) =>
  execFileSync("git", ["-C", fixture, ...args], {
    windowsHide: true,
    encoding: "utf8",
  });
git("init", "--quiet", "--initial-branch=main");
git("config", "user.name", "GitCrab Smoke");
git("config", "user.email", "smoke@example.invalid");
git("config", "core.autocrlf", "false");
await mkdir(join(fixture, "hooks"));
git("config", "core.hooksPath", join(fixture, "hooks"));
git("config", "commit.gpgsign", "false");
await writeFile(join(fixture, "demo.rs"), "// Initial version\n");
git("add", ".");
git("commit", "--quiet", "-m", "Initialize smoke history");
await writeFile(join(fixture, "demo.rs"), "// Changed version\n");
const child = spawn(binary, [fixture], {
  windowsHide: true,
  env: { ...process.env, TAURI_WEBDRIVER_PORT: "4445" },
});
let session;
const delay = (ms) => new Promise((r) => setTimeout(r, ms));
async function command(method, path, body) {
  const response = await fetch(`http://127.0.0.1:4445${path}`, {
    method,
    headers: { "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
    signal: AbortSignal.timeout(20000),
  });
  const result = await response.json();
  if (result.value?.error) throw Error(JSON.stringify(result.value));
  return result.value;
}
try {
  for (let i = 0; i < 100; i++) {
    try {
      await command("GET", "/status");
      break;
    } catch {
      await delay(200);
    }
  }
  session = (
    await command("POST", "/session", { capabilities: { alwaysMatch: {} } })
  ).sessionId;
  const execute = (script) =>
    command("POST", `/session/${session}/execute/sync`, { script, args: [] });
  const wait = async (script) => {
    for (let i = 0; i < 100; i++) {
      if (await execute(script)) return;
      await delay(100);
    }
    throw Error(`Timed out: ${script}`);
  };
  const click = (selector) =>
    execute(
      `document.querySelector(${JSON.stringify(selector)}).click(); return true;`,
    );
  const capture = async (view) => {
    await delay(100);
    const png = await command("GET", `/session/${session}/screenshot`);
    await writeFile(join(output, `${view}.png`), Buffer.from(png, "base64"));
  };
  await wait("return document.querySelectorAll('.history-row').length > 1;");
  await capture("history");
  await click(".history-row:not(.wip)");
  await wait("return !!document.querySelector('.changed-file');");
  await capture("details");
  await click(".changed-file");
  await wait("return !!document.querySelector('.diff-line');");
  await capture("commitdiff");
  await click('[aria-label="Close diff"]');
  await click(".wip");
  await capture("staging");
  await click(".file-section .file-name");
  await wait("return !!document.querySelector('.diff-line');");
  await capture("diff");
  await click('.diff-controls input[type="checkbox"]');
  await wait("return !!document.querySelector('.diff-line.split');");
  await capture("split");
  const viewport = await execute(
    "return {width:innerWidth,height:innerHeight,dpr:devicePixelRatio};",
  );
  await writeFile(
    join(output, "result.json"),
    JSON.stringify(
      {
        binary,
        platform: process.platform,
        viewport,
        passed: true,
        scope:
          "Actual native test-feature WebView: Git load, history, details, committed diff, staging, unified and split render. Production binary compiled separately without the automation plugin.",
      },
      null,
      2,
    ),
  );
  console.log(
    "Native renderer smoke passed",
    process.platform,
    JSON.stringify(viewport),
  );
} finally {
  if (session) await command("DELETE", `/session/${session}`).catch(() => {});
  child.kill();
}
