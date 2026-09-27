export type NetworkDiagnosticRun = {
  generation: number;
  currentId: string | null;
  finishedId: string | null;
  ignoredId: string | null;
  starting: boolean;
};

export function beginNetworkDiagnostic(run: NetworkDiagnosticRun): { generation: number; cancelId: string | null } | null {
  if (run.starting) return null;
  const cancelId = run.currentId;
  run.ignoredId = run.currentId ?? run.finishedId;
  run.currentId = null;
  run.finishedId = null;
  run.starting = true;
  run.generation += 1;
  return { generation: run.generation, cancelId };
}

export function acceptNetworkDiagnosticEvent(run: NetworkDiagnosticRun, id: string, finished: boolean): boolean {
  if (id === run.ignoredId || id === run.finishedId || (run.currentId && id !== run.currentId)) return false;
  run.currentId = finished ? null : id;
  if (finished) {
    run.finishedId = id;
    run.starting = false;
  }
  return true;
}

export function acceptNetworkDiagnosticResponse(run: NetworkDiagnosticRun, generation: number, id: string): boolean {
  if (generation !== run.generation) return false;
  run.starting = false;
  if (id === run.finishedId || (run.currentId && id !== run.currentId)) return false;
  run.currentId = id;
  return true;
}

export function failNetworkDiagnosticStart(run: NetworkDiagnosticRun, generation: number): boolean {
  if (generation !== run.generation) return false;
  run.starting = false;
  run.currentId = null;
  return true;
}
