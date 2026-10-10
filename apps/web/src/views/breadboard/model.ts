/** Physical build guides are separate from the circuit-core op log. */
export const BOARD_ROWS = 63;
export type Jumper = { id: string; from: string; to: string };
export type PartPlacement = { board: number; row: number; package: string; holes: Record<string, string> };
export type BreadboardLayout = { rev: number; boards: number; parts: Record<string, PartPlacement>; jumpers: Jumper[] };

const COLUMNS = "ABCDEFGHIJ";

export function strip(hole: string): string {
  if (hole.startsWith("X:")) return hole;
  const match = /^B([1-8]):([A-J])([1-9]|[1-5]\d|6[0-3])$/.exec(hole);
  if (!match) throw new Error(`Invalid breadboard hole ${hole}`);
  return `B${match[1]}:${COLUMNS.indexOf(match[2]!) < 5 ? "L" : "R"}${match[3]}`;
}

export function holePosition(hole: string): [number, number] {
  if (hole.startsWith("X:")) return [-9.1, -12 + Number(hole.match(/\d+/)?.[0] ?? 1) * 3.4 + (hole.endsWith(".N") ? 0.65 : 0)];
  const match = /^B([1-8]):([A-J])([1-9]|[1-5]\d|6[0-3])$/.exec(hole);
  if (!match) throw new Error(`Invalid breadboard hole ${hole}`);
  const offset = (Number(match[1]) - 1) * 13;
  const column = COLUMNS.indexOf(match[2]!);
  return [offset + (column < 5 ? -4.3 + column * 0.75 : 1.3 + (column - 5) * 0.75), (Number(match[3]) - 32) * 0.48];
}
