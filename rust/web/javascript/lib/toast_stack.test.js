// bun test rust/web/javascript/lib — the flash stack's geometry (toast_stack.js).
import { expect, test } from "bun:test";
import { stackLayout } from "./toast_stack.js";

test("collapsed: the front one's height for all, three peek out, the rest wait", () => {
  const { items, height } = stackLayout([40, 60, 50, 70], false);
  expect(items.map((i) => i.height)).toEqual([40, 40, 40, 40]);
  expect(items.map((i) => i.y + 0)).toEqual([0, -10, -20, -30]);
  expect(items.map((i) => i.opacity)).toEqual([1, 1, 1, 0]);
  expect(items.map((i) => i.back)).toEqual([false, true, true, true]);
  expect(height).toBe(60);
});

test("open: each at its own height, 8px apart", () => {
  const { items, height } = stackLayout([40, 60, 50], true);
  expect(items.map((i) => i.height)).toEqual([40, 60, 50]);
  expect(items.map((i) => i.y + 0)).toEqual([0, -48, -116]);
  expect(items.every((i) => i.scale === 1 && i.opacity === 1 && !i.back)).toBe(true);
  expect(height).toBe(166);
});

test("empty: no box left over the page", () => {
  expect(stackLayout([], false).height).toBe(0);
  expect(stackLayout([], true).height).toBe(0);
});
