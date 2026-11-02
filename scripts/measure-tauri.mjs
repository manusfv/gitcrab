import { chromium } from "@playwright/test";
import { spawn, execFileSync } from "node:child_process";
import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir, cpus, totalmem, release } from "node:os";
import { resolve, join } from "node:path";
const [
  bin = "target/release/gitcrab.exe",
  destination = "evidence/tauri/performance.json",
] = process.argv.slice(2);
const binary = resolve(bin),
  cases = [];
for (const repository of [
  "fixtures/showcase",
  "fixtures/large",
  "fixtures/merge-heavy",
]) {
  const profile = await mkdtemp(join(tmpdir(), "gitcrab-perf-profile-")),
    start = performance.now();
  const child = spawn(binary, [resolve(repository)], {
    windowsHide: true,
    env: {
      ...process.env,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:
        "--remote-debugging-port=9225 --remote-debugging-address=127.0.0.1",
      WEBVIEW2_USER_DATA_FOLDER: profile,
    },
  });
  let browser;
  try {
    for (let i = 0; i < 100; i++) {
      try {
        browser = await chromium.connectOverCDP("http://127.0.0.1:9225");
        break;
      } catch {
        await new Promise((r) => setTimeout(r, 100));
      }
    }
    if (!browser) throw Error("Native app did not start");
    const page = browser.contexts()[0].pages()[0];
    await page.bringToFront();
    await page.getByRole("listbox").waitFor();
    const startup_ms = performance.now() - start,
      total = Number(
        execFileSync(
          "git",
          ["-C", repository, "rev-list", "--all", "--count"],
          { encoding: "utf8", windowsHide: true },
        ).trim(),
      );
    while (
      await page
        .getByRole("button", { name: "Load 500 more", exact: true })
        .count()
    ) {
      const before = await page.locator("footer").innerText();
      await page
        .getByRole("button", { name: "Load 500 more", exact: true })
        .click();
      await page.waitForFunction(
        (before) =>
          !document.querySelector("footer").innerText.includes("Loading…") &&
          (document.querySelector("footer").innerText !== before ||
            !Array.from(document.querySelectorAll("button")).some(
              (b) => b.textContent === "Load 500 more",
            )),
        before,
      );
    }
    const loaded = await page.locator("footer").innerText();
    async function scroll(selector) {
      return page.evaluate(async (selector) => {
        const el = document.querySelector(selector);
        el.scrollTop = 0;
        const samples = [];
        let previous = performance.now();
        for (let i = 0; i < 240; i++) {
          await new Promise((r) => requestAnimationFrame(r));
          const now = performance.now();
          samples.push(now - previous);
          previous = now;
          el.scrollTop =
            (el.scrollTop + 42) %
            Math.max(1, el.scrollHeight - el.clientHeight);
        }
        return {
          samples,
          rendered_rows: document.querySelectorAll(".history-row,.diff-line")
            .length,
          scroll_height: el.scrollHeight,
        };
      }, selector);
    }
    const history = await scroll(".history-viewport");
    await page.getByRole("button", { name: "Refresh", exact: true }).click();
    await page
      .getByRole("button", { name: "Refresh", exact: true })
      .isEnabled();
    await new Promise((r) => setTimeout(r, 800));
    const sample = () =>
      JSON.parse(
        execFileSync(
          "powershell",
          [
            "-NoProfile",
            "-Command",
            "& ([scriptblock]::Create([IO.File]::ReadAllText('scripts/process-sample.ps1'))) -RootProcessId " +
              String(child.pid),
          ],
          { encoding: "utf8", windowsHide: true },
        ),
      );
    const idle_start = sample();
    await new Promise((r) => setTimeout(r, 3000));
    const idle_end = sample();
    const collectFrames = () =>
      page.evaluate(() =>
        performance
          .getEntriesByType("measure")
          .filter((e) => e.name.startsWith("gitcrab."))
          .map((e) => ({
            name: e.name,
            duration_ms: e.duration,
            detail: e.detail,
          })),
      );
    let diff, diff_open_ms;
    if (repository.includes("merge-heavy")) {
      await page
        .getByRole("button", { name: "Working directory", exact: false })
        .click();
      const diffStart = performance.now();
      await page
        .locator(".file-section")
        .first()
        .getByRole("button", { name: "large.rs", exact: true })
        .click();
      await page.getByRole("region", { name: "Diff for large.rs" }).waitFor();
      diff_open_ms = performance.now() - diffStart;
      diff = await scroll(".diff-scroll");
    }
    const summarize = ({ samples, ...other }) => {
      const sorted = [...samples].sort((a, b) => a - b);
      return {
        ...other,
        samples_ms: samples,
        p50_ms: sorted[Math.floor(sorted.length * 0.5)],
        p95_ms: sorted[Math.floor(sorted.length * 0.95)],
        p99_ms: sorted[Math.floor(sorted.length * 0.99)],
        over_16_7_ms: samples.filter((x) => x > 16.7).length,
      };
    };
    cases.push({
      repository,
      total,
      loaded,
      startup_ms,
      viewport: await page.evaluate(() => ({
        width: innerWidth,
        height: innerHeight,
        dpr: devicePixelRatio,
      })),
      history: summarize(history),
      diff: diff ? summarize(diff) : null,
      diff_open_ms,
      roundtrips: await collectFrames(),
      idle: {
        duration_seconds: 3,
        cpu_seconds: idle_end.cpu_seconds - idle_start.cpu_seconds,
        working_set: idle_end.working_set,
        private_bytes: idle_end.private_bytes,
        processes: idle_end.processes,
      },
    });
  } finally {
    await browser?.close();
    child.kill();
  }
}
await mkdir(resolve(destination, ".."), { recursive: true });
await writeFile(
  destination,
  JSON.stringify(
    {
      date: new Date().toISOString(),
      binary,
      hardware: {
        cpu: cpus()[0].model,
        logical_cpus: cpus().length,
        ram_bytes: totalmem(),
        windows: release(),
      },
      conditions:
        "Native release Tauri + WebView2 with local assets; headed CDP instrumentation; fresh WebView profile per fixture, OS file caches NOT flushed; startup includes CDP attach/poll and first loaded DOM; 240 rAF-driven scroll steps per case. Frame intervals are instrumented synthetic scrolling, not a full compositor/input trace. Idle CPU and summed working set/private bytes include all descendant WebView processes; shared pages may be double-counted. Concurrent desktop applications were running. Not a cold-cache or egui comparison.",
      cases,
    },
    null,
    2,
  ),
);
console.log("Wrote", destination);
