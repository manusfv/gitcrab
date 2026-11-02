// Original small lexical highlighter, matching the former native presentation.
// It never changes patch text or coordinates and is not a language grammar.
const keywords = new Set(
  "fn pub let mut impl struct enum use mod if else match return const async await function class def import from true false None Some".split(
    " ",
  ),
);
export function highlight(text: string): Array<{ text: string; kind: string }> {
  return (
    text.match(
      /\/\/.*$|^\s*#.*$|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|[\p{L}_][\p{L}\p{N}_]*|\d+(?:\.\d+)?|./gu,
    ) ?? []
  )
    .map((token) => ({
      text: token,
      kind:
        token.startsWith("//") || token.trimStart().startsWith("#")
          ? "comment"
          : /^['"]/.test(token)
            ? "string"
            : keywords.has(token)
              ? "keyword"
              : /^\d/.test(token)
                ? "number"
                : "",
    }))
    .reduce<Array<{ text: string; kind: string }>>((result, token) => {
      const previous = result.at(-1);
      if (previous && !previous.kind && !token.kind)
        previous.text += token.text;
      else result.push(token);
      return result;
    }, []);
}
