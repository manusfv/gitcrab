"""Exercise a diagnostic desktop binary in an isolated X11 session."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time


def wait_until(predicate, process, label):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        result = predicate()
        if result:
            return result
        exit_code = process.poll()
        if exit_code is not None:
            raise RuntimeError(f"Desktop exited with code {exit_code} during {label}")
        time.sleep(0.05)
    raise RuntimeError(f"Timed out during {label}")


def read_json(path):
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (FileNotFoundError, UnicodeDecodeError, json.JSONDecodeError):
        return None


def main():
    if sys.platform != "linux":
        raise SystemExit("This smoke harness requires Linux X11 and xdotool")
    executable = Path(sys.argv[1]).resolve()
    output = Path(sys.argv[2]).resolve()
    output.mkdir(parents=True, exist_ok=False)
    environment = {key: value for key, value in os.environ.items()
                   if not key.upper().startswith("GIT_")}
    with tempfile.TemporaryDirectory(prefix="gitcrab-renderer-") as directory:
        root = Path(directory)
        repo = root / "repo"
        repo.mkdir()
        environment.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=str(root / "empty-config"))

        def git(*args):
            return subprocess.check_output(["git", "-C", str(repo), *args], env=environment).decode()

        git("init", "--initial-branch=main", "--template=")
        for key, value in [("user.name", "Smoke"), ("user.email", "smoke@example.invalid"),
                           ("commit.gpgsign", "false"), ("core.hooksPath", ".git/hooks")]:
            git("config", key, value)
        (repo / "file.txt").write_text("base\n")
        git("add", "--", "file.txt")
        git("commit", "-m", "base")
        head = git("rev-parse", "HEAD")
        (repo / "file.txt").write_text("edited\n")
        results = []
        settings = root / "settings.json"
        for mode in ["dialog", "diff", "stageddiff"]:
            ready = output / f"{mode}.json"
            trust = output / f"{mode}-trust"
            state = output / f"{mode}-state.json"
            environment.update(GITCRAB_SETTINGS=str(settings), GITCRAB_NATIVE_READY=str(ready),
                               GITCRAB_TRUST_READY=str(trust), GITCRAB_CAPTURE_VIEW=mode,
                               GITCRAB_SMOKE_STATE=str(state))
            def state_is(predicate):
                data = read_json(state)
                return data is not None and predicate(data)
            with (output / f"{mode}.log").open("wb") as log:
                process = subprocess.Popen([str(executable), str(repo)], env=environment, stdout=log, stderr=log)
                try:
                    if mode == "dialog":
                        wait_until(trust.exists, process, "trust dialog")
                        assert not ready.exists(), "A snapshot was loaded before trust confirmation"
                    else:
                        wait_until(lambda: trust.exists() or read_json(ready), process, "trusted repository reopen")
                        assert not trust.exists(), "A trusted repository prompted again after restart"
                    windows = subprocess.check_output(
                        ["xdotool", "search", "--pid", str(process.pid), "--name", "GitCrab"]
                    ).decode().splitlines()
                    if len(windows) != 1:
                        raise RuntimeError(f"Expected one GitCrab window, got {windows}")
                    subprocess.run(["xdotool", "windowactivate", "--sync", windows[0]], check=True)
                    if mode == "dialog":
                        subprocess.run(["xdotool", "key", "--clearmodifiers", "Return"], check=True)
                    data = wait_until(lambda: read_json(ready), process, f"{mode} layout")
                    assert data["loaded"] == 1, data
                    if mode == "dialog":
                        assert data["dialog_layer"], data
                        subprocess.run(["xdotool", "key", "--clearmodifiers", "Escape"], check=True)
                        wait_until(lambda: state_is(lambda s: not s["dialog"] and s["focused"]), process, "cancel focus restoration")
                        subprocess.run(["xdotool", "key", "--clearmodifiers", "Down"], check=True)
                        wait_until(lambda: state_is(lambda s: s["selected"] == head.strip()), process, "keyboard history navigation")
                        assert "feature/review" not in git("branch")
                    else:
                        assert data["diff_rows"], data
                        subprocess.run(["xdotool", "key", "--clearmodifiers", "alt+s"], check=True)
                        expected = "edited\n" if mode == "diff" else "base\n"
                        wait_until(lambda: git("show", ":file.txt") == expected, process, "keyboard index action")
                        wait_until(lambda: state_is(lambda s: not s["busy"] and s["notice"] in (
                            "Staged file.txt", "Unstaged file.txt", "All working changes reviewed", "No staged changes remain"
                        )), process, "mutation completion")
                        assert (repo / "file.txt").read_text() == "edited\n"
                    assert git("rev-parse", "HEAD") == head
                    subprocess.run(["xdotool", "key", "--clearmodifiers", "ctrl+q"], check=True)
                    exit_code = process.wait(timeout=10)
                    if exit_code != 0:
                        raise RuntimeError(f"Desktop exited with code {exit_code} after quitting {mode}")
                    results.append({"mode": mode, "passed": True, "head": head.strip()})
                finally:
                    if process.poll() is None:
                        process.kill()
                        process.wait()
        (output / "results.json").write_text(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
