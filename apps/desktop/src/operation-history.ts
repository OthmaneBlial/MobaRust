export function retainOperationHistory<T>(
  entries: readonly T[],
  isFinished: (entry: T) => boolean,
  finishedLimit: number,
): T[] {
  let finished = 0;
  return [...entries].reverse()
    .filter((entry) => !isFinished(entry) || ++finished <= finishedLimit)
    .reverse();
}
