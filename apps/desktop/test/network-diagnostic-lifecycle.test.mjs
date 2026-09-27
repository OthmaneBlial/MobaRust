import assert from "node:assert/strict";
import {
  acceptNetworkDiagnosticEvent,
  acceptNetworkDiagnosticResponse,
  beginNetworkDiagnostic,
  failNetworkDiagnosticStart,
  requestNetworkDiagnosticCancel,
  takePendingNetworkDiagnosticCancel,
} from "../src/network-diagnostic-lifecycle.ts";

const run = { generation: 0, currentId: null, finishedId: null, ignoredId: null, starting: false, cancelRequested: false };
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

const eventFirst = { generation: 0, currentId: null, finishedId: null, ignoredId: null, starting: false, cancelRequested: false };
const eventStart = beginNetworkDiagnostic(eventFirst);
assert.equal(requestNetworkDiagnosticCancel(eventFirst), null);
assert.equal(acceptNetworkDiagnosticEvent(eventFirst, "event-first", false), true);
assert.equal(takePendingNetworkDiagnosticCancel(eventFirst), "event-first", "cancel must run when the event reveals the ID");
assert.equal(takePendingNetworkDiagnosticCancel(eventFirst), null, "cancel must only be sent once");
assert.equal(acceptNetworkDiagnosticResponse(eventFirst, eventStart.generation, "event-first"), true);

const responseFirst = { generation: 0, currentId: null, finishedId: null, ignoredId: null, starting: false, cancelRequested: false };
const responseStart = beginNetworkDiagnostic(responseFirst);
assert.equal(requestNetworkDiagnosticCancel(responseFirst), null);
assert.equal(acceptNetworkDiagnosticResponse(responseFirst, responseStart.generation, "response-first"), true);
assert.equal(takePendingNetworkDiagnosticCancel(responseFirst), "response-first", "cancel must run when the response reveals the ID");

const alreadyFinished = { generation: 0, currentId: null, finishedId: null, ignoredId: null, starting: false, cancelRequested: false };
beginNetworkDiagnostic(alreadyFinished);
requestNetworkDiagnosticCancel(alreadyFinished);
acceptNetworkDiagnosticEvent(alreadyFinished, "already-finished", true);
assert.equal(takePendingNetworkDiagnosticCancel(alreadyFinished), null, "a finished operation needs no cancel");
