import assert from "node:assert/strict";
import {
  acceptNetworkDiagnosticEvent,
  acceptNetworkDiagnosticResponse,
  beginNetworkDiagnostic,
  failNetworkDiagnosticStart,
} from "../src/network-diagnostic-lifecycle.ts";

const run = { generation: 0, currentId: null, finishedId: null, ignoredId: null, starting: false };
const first = beginNetworkDiagnostic(run);
assert.equal(first?.generation, 1);
assert.equal(beginNetworkDiagnostic(run), null, "a second start must wait for the first operation ID");
assert.equal(acceptNetworkDiagnosticEvent(run, "fast-ping", false), true);
assert.equal(acceptNetworkDiagnosticEvent(run, "fast-ping", true), true);
assert.equal(acceptNetworkDiagnosticResponse(run, first.generation, "fast-ping"), false, "a late response must not revive a completed operation");
assert.equal(run.currentId, null);

const second = beginNetworkDiagnostic(run);
assert.equal(acceptNetworkDiagnosticResponse(run, first.generation, "fast-ping"), false, "a previous response must not replace a new run");
assert.equal(acceptNetworkDiagnosticEvent(run, "fast-ping", true), false, "old events must not replace the new run");
assert.equal(acceptNetworkDiagnosticResponse(run, second.generation, "traceroute"), true);
assert.equal(acceptNetworkDiagnosticEvent(run, "traceroute", false), true);
assert.equal(acceptNetworkDiagnosticEvent(run, "traceroute", true), true);
assert.equal(acceptNetworkDiagnosticEvent(run, "traceroute", false), false, "progress after completion must be ignored");

const third = beginNetworkDiagnostic(run);
assert.equal(failNetworkDiagnosticStart(run, third.generation), true);
assert.equal(run.starting, false);
