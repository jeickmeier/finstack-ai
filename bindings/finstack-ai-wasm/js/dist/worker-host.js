import { FinstackError } from "./errors.js";
import { encodeControl, decodeMainToWorker, } from "./worker-protocol.js";
/**
 * Serve the worker protocol from a Dedicated Worker.
 *
 * Call this from the worker module after constructing adapters locally.
 * The UI thread must not pass `JsModel` or other host objects across the
 * boundary.
 *
 * @param factory - Creates one Agent from a serializable options payload.
 * @example
 * ```ts
 * exposeWorkerHost({
 *   async create() {
 *     return Agent.create({ model });
 *   },
 * });
 * ```
 */
export function exposeWorkerHost(factory) {
    const scope = globalThis;
    const agents = new Map();
    const runs = new Map();
    let nextAgent = 0;
    let defaults = {
        policy: "drop-progress",
        queueCapacity: 32,
        blockTimeoutMs: 2000,
        durableTimeoutMs: 2000,
    };
    scope.addEventListener("message", (event) => {
        void handle(event.data);
    });
    async function handle(data) {
        let requestId;
        try {
            await dispatch(data, (id) => {
                requestId = id;
            });
        }
        catch (error) {
            postError(undefined, FinstackError.fromUnknown(error), requestId);
        }
    }
    async function dispatch(data, captureId) {
        const message = decodeMainToWorker(data);
        if ("id" in message && message.id !== undefined) {
            captureId(message.id);
        }
        switch (message.type) {
            case "init":
                if (message.lagPolicy !== undefined) {
                    defaults.policy = message.lagPolicy;
                }
                if (message.queueCapacity !== undefined && message.queueCapacity > 0) {
                    defaults.queueCapacity = message.queueCapacity;
                }
                if (message.blockTimeoutMs !== undefined && message.blockTimeoutMs > 0) {
                    defaults.blockTimeoutMs = message.blockTimeoutMs;
                }
                if (message.durableTimeoutMs !== undefined && message.durableTimeoutMs > 0) {
                    defaults.durableTimeoutMs = message.durableTimeoutMs;
                }
                post({ v: 1, type: "ready", id: message.id });
                return;
            case "create": {
                const agent = await factory.create(message.options);
                const agentId = `js.agent.${nextAgent}`;
                nextAgent += 1;
                agents.set(agentId, agent);
                post({ v: 1, type: "created", id: message.id, agentId });
                return;
            }
            case "start": {
                const agent = agents.get(message.agentId);
                if (agent === undefined) {
                    throw new FinstackError("unknown worker agent", {
                        code: "agent_run_invalid_configuration",
                        retryable: false,
                    });
                }
                const run = agent.start(message.input, message.options);
                const session = run.session.toDict();
                const slot = {
                    agentId: message.agentId,
                    runId: session.runId,
                    run,
                    session,
                    policy: defaults.policy,
                    queueCapacity: defaults.queueCapacity,
                    blockTimeoutMs: defaults.blockTimeoutMs,
                    durableTimeoutMs: defaults.durableTimeoutMs,
                    pending: 0,
                    dropped: 0,
                    waiters: [],
                    closed: false,
                };
                runs.set(session.runId, slot);
                post({
                    v: 1,
                    type: "started",
                    id: message.id,
                    agentId: message.agentId,
                    runId: session.runId,
                    session,
                });
                void pump(slot);
                return;
            }
            case "cancel": {
                const slot = requireRun(message.agentId, message.runId);
                await slot.run.cancel(message.reason);
                post({ v: 1, type: "ready", id: message.id });
                return;
            }
            case "closeEvents": {
                const slot = requireRun(message.agentId, message.runId);
                slot.closed = true;
                await slot.run.closeEvents();
                post({ v: 1, type: "ready", id: message.id });
                return;
            }
            case "ack": {
                const slot = requireRun(message.agentId, message.runId);
                if (slot.pending > 0) {
                    slot.pending -= 1;
                }
                const waiter = slot.waiters.shift();
                waiter?.();
                return;
            }
            case "inspect": {
                if (factory.inspectSession === undefined) {
                    throw new FinstackError("worker inspect is unavailable", {
                        code: "agent_run_invalid_configuration",
                        retryable: false,
                    });
                }
                const snapshot = await factory.inspectSession(message.sessionId);
                post({
                    v: 1,
                    type: "inspected",
                    id: message.id,
                    snapshot: {
                        ...snapshot,
                        headSequence: Number(snapshot.headSequence),
                    },
                });
                return;
            }
            case "shutdown":
                for (const slot of runs.values()) {
                    slot.closed = true;
                    await slot.run.cancel("worker_shutdown").catch(() => undefined);
                }
                post({ v: 1, type: "terminated", reason: "shutdown" });
                return;
            default: {
                const _exhaustive = message;
                throw new FinstackError(`unsupported worker command: ${String(_exhaustive)}`, {
                    code: "agent_run_invalid_configuration",
                    retryable: false,
                });
            }
        }
    }
    async function pump(slot) {
        try {
            for await (const batch of slot.run.events()) {
                if (slot.closed) {
                    break;
                }
                const durable = batch
                    .events()
                    .some((event) => event.eventClass === "durable_derived");
                const accepted = await enqueue(slot, batch, durable);
                if (accepted === "disconnected") {
                    postError(slot, new FinstackError("worker event subscription disconnected", {
                        code: "agent_run_runtime_failure",
                        retryable: false,
                        context: slot.session,
                    }));
                    await slot.run.closeEvents();
                    break;
                }
            }
            const result = await slot.run.result();
            post({
                v: 1,
                type: "result",
                agentId: slot.agentId,
                runId: slot.runId,
                snapshot: result.toDict(),
            });
        }
        catch (error) {
            postError(slot, FinstackError.fromUnknown(error));
        }
    }
    async function enqueue(slot, batch, durable) {
        const timeoutMs = durable ? slot.durableTimeoutMs : slot.blockTimeoutMs;
        const started = Date.now();
        while (slot.pending >= slot.queueCapacity) {
            if (slot.policy === "disconnect") {
                return "disconnected";
            }
            if (slot.policy === "drop-progress" && !durable) {
                slot.dropped += Math.max(1, batch.lastSequence - batch.firstSequence + 1);
                publishBackpressure(slot);
                return "dropped";
            }
            const remaining = timeoutMs - (Date.now() - started);
            if (remaining <= 0) {
                if (durable) {
                    break;
                }
                if (slot.policy === "drop-progress") {
                    slot.dropped += Math.max(1, batch.lastSequence - batch.firstSequence + 1);
                    publishBackpressure(slot);
                    return "dropped";
                }
                return "disconnected";
            }
            await waitForAck(slot, remaining);
        }
        const bytes = copyBuffer(batch.toJsonBytes());
        const droppedProgress = batch.droppedProgress + slot.dropped;
        slot.dropped = 0;
        slot.pending += 1;
        post({
            v: 1,
            type: "batch",
            agentId: slot.agentId,
            runId: slot.runId,
            firstSequence: batch.firstSequence,
            lastSequence: batch.lastSequence,
            droppedProgress,
            durable,
        }, [bytes]);
        publishBackpressure(slot);
        return "sent";
    }
    function waitForAck(slot, timeoutMs) {
        return new Promise((resolve) => {
            const timer = setTimeout(() => {
                const index = slot.waiters.indexOf(onAck);
                if (index >= 0) {
                    slot.waiters.splice(index, 1);
                }
                resolve();
            }, timeoutMs);
            const onAck = () => {
                clearTimeout(timer);
                resolve();
            };
            slot.waiters.push(onAck);
        });
    }
    function publishBackpressure(slot) {
        post({
            v: 1,
            type: "backpressure",
            agentId: slot.agentId,
            runId: slot.runId,
            queuedBatches: slot.pending,
            droppedProgress: slot.dropped,
            policy: slot.policy,
        });
    }
    function requireRun(agentId, runId) {
        const slot = runs.get(runId);
        if (slot === undefined || slot.agentId !== agentId) {
            throw new FinstackError("unknown worker run", {
                code: "agent_run_invalid_configuration",
                retryable: false,
            });
        }
        return slot;
    }
    function post(message, transfer) {
        const encoded = JSON.parse(encodeControl(message));
        if (transfer !== undefined) {
            scope.postMessage({ envelope: encoded, bytes: transfer[0] }, transfer);
            return;
        }
        scope.postMessage(encoded);
    }
    function postError(slot, error, requestId) {
        const message = {
            v: 1,
            type: "error",
            code: error.code,
            retryable: error.retryable,
            message: error.message,
        };
        if (requestId !== undefined) {
            message.id = requestId;
        }
        if (slot !== undefined) {
            message.agentId = slot.agentId;
            message.runId = slot.runId;
            message.context = slot.session;
        }
        if (error.context !== undefined && message.context === undefined) {
            message.context = error.context;
        }
        post(message);
    }
}
function copyBuffer(bytes) {
    const copy = new ArrayBuffer(bytes.byteLength);
    new Uint8Array(copy).set(bytes);
    return copy;
}
//# sourceMappingURL=worker-host.js.map