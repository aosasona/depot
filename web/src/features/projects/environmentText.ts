export type EnvironmentEntry = {
  name: string;
  value: string;
  is_secret: boolean;
};

/** Parses a bounded dotenv-style block without expanding variables or executing syntax. */
export function parseEnvironmentText(
  input: string,
  isSecret: boolean,
): EnvironmentEntry[] {
  const entries: EnvironmentEntry[] = [];
  const names = new Set<string>();
  for (const [index, sourceLine] of input.split(/\r?\n/).entries()) {
    let line = sourceLine.trim();
    if (!line || line.startsWith("#")) continue;
    if (line.startsWith("export ")) line = line.slice(7).trimStart();
    const separator = line.indexOf("=");
    if (separator <= 0)
      throw new Error(`Line ${index + 1} must use KEY=value.`);
    const name = line.slice(0, separator).trim();
    if (!/^[A-Z_][A-Z0-9_]{0,127}$/.test(name))
      throw new Error(
        `Line ${index + 1} has an invalid key. Use uppercase letters, numbers, and underscores.`,
      );
    if (names.has(name)) throw new Error(`${name} is defined more than once.`);
    let value = line.slice(separator + 1).trim();
    if (
      value.length >= 2 &&
      ((value.startsWith('"') && value.endsWith('"')) ||
        (value.startsWith("'") && value.endsWith("'")))
    )
      value = value.slice(1, -1);
    if (value.length > 32_768)
      throw new Error(`${name} exceeds the 32 KiB value limit.`);
    names.add(name);
    entries.push({ name, value, is_secret: isSecret });
  }
  if (entries.length > 128)
    throw new Error("A project can contain at most 128 environment values.");
  return entries;
}
