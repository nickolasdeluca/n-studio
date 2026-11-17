// What a script touches, by object kind, as the Script Executive's
// explorer shows it: built from the statements' subjects, without running
// anything. Kept free of imports so its tests run under plain node.
import type { Operation, ScriptBlock, SubjectKind } from "./api";

/** One statement under an object */
export interface TreeItem {
  index: number;
  operation: Operation;
  /** Below the object: ".COLUMN" for a comment on a column, an index's
   *  name under its table */
  detail: string | null;
  /** Under a table because of an index on it */
  index_on: boolean;
}

export interface TreeObject {
  name: string;
  items: TreeItem[];
}

export interface TreeGroup {
  kind: SubjectKind;
  objects: TreeObject[];
  /** Statements of the kind with no object to go under: blocks, the
   *  database, a grant of something unnamed */
  loose: TreeItem[];
}

/** The kinds in the order the tree lists them; every one is listed, those
 *  with nothing dimmed, so the tree does not jump as the script changes */
export const treeKinds: SubjectKind[] = [
  "table",
  "view",
  "procedure",
  "function",
  "package",
  "trigger",
  "domain",
  "generator",
  "exception",
  "index",
  "role",
  "user",
  "udf",
  "block",
  "database",
];

export function scriptTree(blocks: ScriptBlock[]): TreeGroup[] {
  const groups = new Map<SubjectKind, { objects: Map<string, TreeObject>; loose: TreeItem[] }>(
    treeKinds.map((kind) => [kind, { objects: new Map(), loose: [] }]),
  );
  const add = (kind: SubjectKind, name: string | null, item: TreeItem) => {
    const group = groups.get(kind);
    if (!group) return;
    if (name === null) {
      group.loose.push(item);
      return;
    }
    let object = group.objects.get(name);
    if (!object) {
      object = { name, items: [] };
      group.objects.set(name, object);
    }
    object.items.push(item);
  };
  for (const block of blocks) {
    const { operation, kind, name, member } = block.subject;
    if (!kind) continue;
    const item = (detail: string | null, index_on = false): TreeItem => ({ index: block.index, operation, detail, index_on });
    if (kind === "index") {
      add("index", name, item(null));
      // And under the table it is on
      if (member) add("table", member, item(name, true));
    } else if (kind === "trigger") {
      add("trigger", name, item(null));
    } else {
      add(kind, name, item(member === null ? null : `.${member}`));
    }
  }
  return treeKinds.map((kind) => {
    const group = groups.get(kind)!;
    return { kind, objects: [...group.objects.values()], loose: group.loose };
  });
}
