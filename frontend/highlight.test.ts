import { expect, it } from "vitest";
import { highlight } from "./highlight";
it("retains exact source including Unicode and quoted comment markers", () => {
  const source = 'let 名 = "https://example.test/ü"; // note 42';
  const tokens = highlight(source);
  expect(tokens.map((t) => t.text).join("")).toBe(source);
  expect(tokens.find((t) => t.text === "let")?.kind).toBe("keyword");
  expect(tokens.find((t) => t.text === '"https://example.test/ü"')?.kind).toBe(
    "string",
  );
  expect(tokens.at(-1)?.kind).toBe("comment");
});
