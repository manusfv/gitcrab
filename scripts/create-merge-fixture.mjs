import { execFileSync } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import { resolve, join } from "node:path";
const root = resolve(process.argv[2] ?? "fixtures/merge-heavy");
// mkdir without recursive rejects an existing destination before any Git write.
await mkdir(root);
const git = (...args) =>
  execFileSync("git", ["-C", root, ...args], {
    windowsHide: true,
    encoding: "utf8",
  });
git("init", "--quiet", "--initial-branch=main");
git("config", "core.autocrlf", "false");
let stream = "",
  mark = 0,
  main = 0;
function commit(branch, parent, merge, message, file, body) {
  const id = ++mark,
    time = 1704067200 + id * 60;
  stream += `commit refs/heads/${branch}\nmark :${id}\ncommitter GitCrab Benchmark <benchmark@example.invalid> ${time} +0000\ndata ${message.length}\n${message}\n`;
  if (parent) stream += `from :${parent}\n`;
  if (merge) stream += `merge :${merge}\n`;
  stream += `M 100644 inline ${file}\ndata ${body.length}\n${body}\n`;
  return id;
}
main = commit(
  "main",
  0,
  0,
  "Initialize large file",
  "large.rs",
  Array.from({ length: 20000 }, (_, i) => `// Original line ${i}`).join("\n") +
    "\n",
);
for (let i = 0; i < 1000; i++) {
  const side = commit(
    `feature/${i % 12}`,
    main,
    0,
    `Feature ${i}`,
    "feature.txt",
    `${i}\n`,
  );
  main = commit("main", main, 0, `Main ${i}`, "main.txt", `${i}\n`);
  main = commit(
    "main",
    main,
    side,
    `Merge feature ${i}`,
    "merge.txt",
    `${i}\n`,
  );
}
stream += "done\n";
execFileSync("git", ["-C", root, "fast-import", "--quiet"], {
  input: stream,
  windowsHide: true,
});
// This hard reset only materializes the exclusively created fixture above.
git("reset", "--hard", "--quiet", "HEAD");
await writeFile(
  join(root, "large.rs"),
  Array.from({ length: 20000 }, (_, i) => `// Replacement line ${i}`).join(
    "\n",
  ) + "\n",
);
console.log(
  `Created ${root}: ${mark} commits, 1000 merges, 20,000-line replacement diff`,
);
