/**
 * Shared `--flag=value` argv parsing. Splitting on the first `=` only (not
 * `String.split('=')[1]`, which truncates at the *second* `=` too) matters
 * for any flag whose value can itself contain `=` — a file path is the
 * common case (e.g. `--output=./results/a=b.json` on a filesystem that
 * allows it, or a path copied from a URL query string).
 */
export function argValue(args: string[], flag: string): string | undefined {
  const prefix = `${flag}=`;
  const arg = args.find((a) => a.startsWith(prefix));
  return arg === undefined ? undefined : arg.slice(prefix.length);
}
