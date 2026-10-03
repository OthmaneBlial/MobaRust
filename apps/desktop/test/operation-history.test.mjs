import assert from "node:assert/strict";
import console from "node:console";
import { retainOperationHistory } from "../src/operation-history.ts";

for (const [activeStates, finishedStates, limit] of [
  [["listening", "running", "stopping"], ["stopped", "failed"], 20],
  [["queued", "preparing", "running", "paused", "cancelling"], ["completed", "cancelled", "failed"], 40],
]) {
  const isFinished = (entry) => finishedStates.includes(entry.state);
  const live = Array.from({ length: limit + 12 }, (_, id) => ({ id, state: activeStates[id % activeStates.length] }));
  assert.deepEqual(retainOperationHistory(live, isFinished, limit), live,
    "active operations must keep their Stop/Cancel controls even beyond the history limit");
  const finished = Array.from({ length: limit + 10 }, (_, index) => ({ id: `finished-${index}`, state: finishedStates[index % finishedStates.length] }));
  const mixed = live.flatMap((entry, index) => [entry, { id: `history-${index}`, state: finishedStates[index % finishedStates.length] }]);
  const original = mixed.slice();
  const retained = retainOperationHistory(mixed, isFinished, limit);
  assert.deepEqual(retained.filter((entry) => !isFinished(entry)), live);
  assert.deepEqual(retained.filter(isFinished), mixed.filter(isFinished).slice(-limit),
    "retain only the most recently updated finished history");
  assert.deepEqual(mixed, original, "retention must not mutate React's current array");
  assert.deepEqual(retained, mixed.filter((entry) => retained.includes(entry)), "preserve event order");
  assert.deepEqual(retainOperationHistory(mixed, isFinished, 0), live);
  assert.deepEqual(retainOperationHistory(finished, isFinished, limit), finished.slice(-limit));
  const completed = live.map((entry) => ({ ...entry, state: finishedStates[0] }));
  assert.deepEqual(retainOperationHistory(completed, isFinished, limit), completed.slice(-limit),
    "finished operations must return to bounded history instead of accumulating forever");
  assert.deepEqual(retainOperationHistory([], isFinished, limit), []);
}

console.log("Active tunnel/transfer history retention checks passed");
