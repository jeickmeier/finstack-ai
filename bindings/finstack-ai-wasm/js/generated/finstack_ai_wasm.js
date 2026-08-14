/* @ts-self-types="./finstack_ai_wasm.d.ts" */

/**
 * Rust-owned Agent handle.
 */
export class Agent {
    static __wrap(ptr) {
        const obj = Object.create(Agent.prototype);
        obj.__wbg_ptr = ptr;
        AgentFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        AgentFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_agent_free(ptr, 0);
    }
    /**
     * Return the bounded model-activated capability catalog in identity order.
     *
     * # Errors
     *
     * Returns a JavaScript exception when the catalog object cannot be constructed.
     * @returns {any}
     */
    capabilityCatalog() {
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.agent_capabilityCatalog(retptr, this.__wbg_ptr);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            var r2 = getDataViewMemory0().getInt32(retptr + 4 * 2, true);
            if (r2) {
                throw takeObject(r1);
            }
            return takeObject(r0);
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
        }
    }
    /**
     * Render the compact catalog supplied to model-facing integrations.
     * @returns {string}
     */
    compactCapabilityCatalog() {
        let deferred1_0;
        let deferred1_1;
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.agent_compactCapabilityCatalog(retptr, this.__wbg_ptr);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            deferred1_0 = r0;
            deferred1_1 = r1;
            return getStringFromWasm0(r0, r1);
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
            wasm.__wbindgen_export5(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Construct an Agent over a trusted JS model and optional toolsets.
     *
     * # Errors
     *
     * Returns a structured host error when configuration is invalid.
     * @param {JsModel} model
     * @param {JsToolset[]} toolsets
     * @param {string | null} [instruction]
     * @param {JsJournalStore | null} [store]
     * @param {string | null} [capabilities_json]
     * @param {string | null} [active_capabilities_json]
     * @returns {Promise<any>}
     */
    static create(model, toolsets, instruction, store, capabilities_json, active_capabilities_json) {
        _assertClass(model, JsModel);
        const ptr0 = passArrayJsValueToWasm0(toolsets, wasm.__wbindgen_export);
        const len0 = WASM_VECTOR_LEN;
        var ptr1 = isLikeNone(instruction) ? 0 : passStringToWasm0(instruction, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        var len1 = WASM_VECTOR_LEN;
        let ptr2 = 0;
        if (!isLikeNone(store)) {
            _assertClass(store, JsJournalStore);
            ptr2 = store.__destroy_into_raw();
        }
        var ptr3 = isLikeNone(capabilities_json) ? 0 : passStringToWasm0(capabilities_json, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        var len3 = WASM_VECTOR_LEN;
        var ptr4 = isLikeNone(active_capabilities_json) ? 0 : passStringToWasm0(active_capabilities_json, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        var len4 = WASM_VECTOR_LEN;
        const ret = wasm.agent_create(model.__wbg_ptr, ptr0, len0, ptr1, len1, ptr2, ptr3, len3, ptr4, len4);
        return takeObject(ret);
    }
    /**
     * Create a live session on this agent's journal store.
     *
     * # Errors
     *
     * Returns a structured host error when the session cannot be created.
     * @param {string | null} [tenant_scope]
     * @returns {Promise<any>}
     */
    createSession(tenant_scope) {
        var ptr0 = isLikeNone(tenant_scope) ? 0 : passStringToWasm0(tenant_scope, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        var len0 = WASM_VECTOR_LEN;
        const ret = wasm.agent_createSession(this.__wbg_ptr, ptr0, len0);
        return takeObject(ret);
    }
    /**
     * Replay one stored session into a provisional inspect snapshot.
     *
     * This does not continue an interrupted run or retry in-flight effects.
     *
     * # Errors
     *
     * Returns a structured host error when the session id is invalid or the
     * stored journal cannot be replayed.
     * @param {JsJournalStore} store
     * @param {string} session_id
     * @returns {Promise<any>}
     */
    static inspectSession(store, session_id) {
        _assertClass(store, JsJournalStore);
        const ptr0 = passStringToWasm0(session_id, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.agent_inspectSession(store.__wbg_ptr, ptr0, len0);
        return takeObject(ret);
    }
    /**
     * Open an existing session without respawning parked runs.
     *
     * # Errors
     *
     * Returns a structured host error when the session id is invalid or the
     * stored journal cannot be replayed.
     * @param {string} session_id
     * @param {string | null} [tenant_scope]
     * @returns {Promise<any>}
     */
    openSession(session_id, tenant_scope) {
        const ptr0 = passStringToWasm0(session_id, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len0 = WASM_VECTOR_LEN;
        var ptr1 = isLikeNone(tenant_scope) ? 0 : passStringToWasm0(tenant_scope, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        var len1 = WASM_VECTOR_LEN;
        const ret = wasm.agent_openSession(this.__wbg_ptr, ptr0, len0, ptr1, len1);
        return takeObject(ret);
    }
    /**
     * Execute one run and await its committed result.
     *
     * # Errors
     *
     * Returns a structured host error when the run fails.
     * @param {string} input
     * @param {number | null} [timeout_seconds]
     * @param {number | null} [max_cycles]
     * @param {number | null} [max_output_retries]
     * @returns {Promise<any>}
     */
    run(input, timeout_seconds, max_cycles, max_output_retries) {
        const ptr0 = passStringToWasm0(input, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.agent_run(this.__wbg_ptr, ptr0, len0, !isLikeNone(timeout_seconds), isLikeNone(timeout_seconds) ? 0 : timeout_seconds, !isLikeNone(max_cycles), isLikeNone(max_cycles) ? 0 : max_cycles, !isLikeNone(max_output_retries), isLikeNone(max_output_retries) ? 0 : max_output_retries);
        return takeObject(ret);
    }
    /**
     * Start one run and return its detached control handle.
     *
     * # Errors
     *
     * Returns a structured host error when the request is invalid.
     * @param {string} input
     * @param {number | null} [timeout_seconds]
     * @param {number | null} [max_cycles]
     * @param {number | null} [max_output_retries]
     * @returns {Run}
     */
    start(input, timeout_seconds, max_cycles, max_output_retries) {
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            const ptr0 = passStringToWasm0(input, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            const len0 = WASM_VECTOR_LEN;
            wasm.agent_start(retptr, this.__wbg_ptr, ptr0, len0, !isLikeNone(timeout_seconds), isLikeNone(timeout_seconds) ? 0 : timeout_seconds, !isLikeNone(max_cycles), isLikeNone(max_cycles) ? 0 : max_cycles, !isLikeNone(max_output_retries), isLikeNone(max_output_retries) ? 0 : max_output_retries);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            var r2 = getDataViewMemory0().getInt32(retptr + 4 * 2, true);
            if (r2) {
                throw takeObject(r1);
            }
            return Run.__wrap(r0);
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
        }
    }
}
if (Symbol.dispose) Agent.prototype[Symbol.dispose] = Agent.prototype.free;

/**
 * Immutable runtime event handle.
 */
export class Event {
    static __wrap(ptr) {
        const obj = Object.create(Event.prototype);
        obj.__wbg_ptr = ptr;
        EventFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        EventFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_event_free(ptr, 0);
    }
    /**
     * Durable sequence, when the event is durable-derived.
     * @returns {bigint | undefined}
     */
    get durableSequence() {
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.event_durableSequence(retptr, this.__wbg_ptr);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r2 = getDataViewMemory0().getBigInt64(retptr + 8 * 1, true);
            return r0 === 0 ? undefined : BigInt.asUintN(64, r2);
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
        }
    }
    /**
     * Durable or transient class.
     * @returns {string}
     */
    get eventClass() {
        let deferred1_0;
        let deferred1_1;
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.event_eventClass(retptr, this.__wbg_ptr);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            deferred1_0 = r0;
            deferred1_1 = r1;
            return getStringFromWasm0(r0, r1);
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
            wasm.__wbindgen_export5(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Event kind name.
     * @returns {string}
     */
    get kind() {
        let deferred1_0;
        let deferred1_1;
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.event_kind(retptr, this.__wbg_ptr);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            deferred1_0 = r0;
            deferred1_1 = r1;
            return getStringFromWasm0(r0, r1);
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
            wasm.__wbindgen_export5(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Explicit JSON snapshot.
     *
     * # Errors
     *
     * Returns a JavaScript exception when the event cannot be serialized.
     * @returns {string}
     */
    toJson() {
        let deferred2_0;
        let deferred2_1;
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.event_toJson(retptr, this.__wbg_ptr);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            var r2 = getDataViewMemory0().getInt32(retptr + 4 * 2, true);
            var r3 = getDataViewMemory0().getInt32(retptr + 4 * 3, true);
            var ptr1 = r0;
            var len1 = r1;
            if (r3) {
                ptr1 = 0; len1 = 0;
                throw takeObject(r2);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
            wasm.__wbindgen_export5(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * Transient sequence.
     * @returns {bigint}
     */
    get transientSequence() {
        const ret = wasm.event_transientSequence(this.__wbg_ptr);
        return BigInt.asUintN(64, ret);
    }
}
if (Symbol.dispose) Event.prototype[Symbol.dispose] = Event.prototype.free;

/**
 * Bounded transport batch. Expand events only on request.
 */
export class EventBatch {
    static __wrap(ptr) {
        const obj = Object.create(EventBatch.prototype);
        obj.__wbg_ptr = ptr;
        EventBatchFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        EventBatchFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_eventbatch_free(ptr, 0);
    }
    /**
     * Lag-dropped transient events since the previous batch.
     * @returns {bigint}
     */
    get droppedProgress() {
        const ret = wasm.eventbatch_droppedProgress(this.__wbg_ptr);
        return BigInt.asUintN(64, ret);
    }
    /**
     * Expand contained events. This is the per-event FFI boundary.
     * @returns {Event[]}
     */
    events() {
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.eventbatch_events(retptr, this.__wbg_ptr);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            var v1 = getArrayJsValueFromWasm0(r0, r1).slice();
            wasm.__wbindgen_export5(r0, r1 * 4, 4);
            return v1;
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
        }
    }
    /**
     * First contained sequence.
     * @returns {bigint}
     */
    get firstSequence() {
        const ret = wasm.eventbatch_firstSequence(this.__wbg_ptr);
        return BigInt.asUintN(64, ret);
    }
    /**
     * Last contained sequence.
     * @returns {bigint}
     */
    get lastSequence() {
        const ret = wasm.eventbatch_lastSequence(this.__wbg_ptr);
        return BigInt.asUintN(64, ret);
    }
    /**
     * Explicit JSON snapshot of the contained events.
     *
     * # Errors
     *
     * Returns a JavaScript exception when the batch cannot be serialized.
     * @returns {string}
     */
    toJson() {
        let deferred2_0;
        let deferred2_1;
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.eventbatch_toJson(retptr, this.__wbg_ptr);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            var r2 = getDataViewMemory0().getInt32(retptr + 4 * 2, true);
            var r3 = getDataViewMemory0().getInt32(retptr + 4 * 3, true);
            var ptr1 = r0;
            var len1 = r1;
            if (r3) {
                ptr1 = 0; len1 = 0;
                throw takeObject(r2);
            }
            deferred2_0 = ptr1;
            deferred2_1 = len1;
            return getStringFromWasm0(ptr1, len1);
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
            wasm.__wbindgen_export5(deferred2_0, deferred2_1, 1);
        }
    }
    /**
     * Explicit UTF-8 JSON bytes of the contained events.
     *
     * # Errors
     *
     * Returns a JavaScript exception when the batch cannot be serialized.
     * @returns {Uint8Array}
     */
    toJsonBytes() {
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.eventbatch_toJsonBytes(retptr, this.__wbg_ptr);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            var r2 = getDataViewMemory0().getInt32(retptr + 4 * 2, true);
            if (r2) {
                throw takeObject(r1);
            }
            return takeObject(r0);
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
        }
    }
}
if (Symbol.dispose) EventBatch.prototype[Symbol.dispose] = EventBatch.prototype.free;

/**
 * Host artifact-store wrapper over `Uint8Array` payloads.
 */
export class JsArtifactStore {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        JsArtifactStoreFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_jsartifactstore_free(ptr, 0);
    }
    /**
     * Construct an artifact-store wrapper around a trusted host adapter.
     *
     * # Errors
     *
     * Returns a TypeError-equivalent when `stagePut` or `get` is missing.
     * @param {any} adapter
     */
    constructor(adapter) {
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.jsartifactstore_new(retptr, addHeapObject(adapter));
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            var r2 = getDataViewMemory0().getInt32(retptr + 4 * 2, true);
            if (r2) {
                throw takeObject(r1);
            }
            this.__wbg_ptr = r0;
            JsArtifactStoreFinalization.register(this, this.__wbg_ptr, this);
            return this;
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
        }
    }
}
if (Symbol.dispose) JsArtifactStore.prototype[Symbol.dispose] = JsArtifactStore.prototype.free;

/**
 * Host clock wrapper. `now()` returns Unix milliseconds.
 */
export class JsClock {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        JsClockFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_jsclock_free(ptr, 0);
    }
    /**
     * Construct a clock wrapper around a trusted host adapter.
     *
     * # Errors
     *
     * Returns a TypeError-equivalent when `now` is missing.
     * @param {any} adapter
     */
    constructor(adapter) {
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.jsclock_new(retptr, addHeapObject(adapter));
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            var r2 = getDataViewMemory0().getInt32(retptr + 4 * 2, true);
            if (r2) {
                throw takeObject(r1);
            }
            this.__wbg_ptr = r0;
            JsClockFinalization.register(this, this.__wbg_ptr, this);
            return this;
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
        }
    }
}
if (Symbol.dispose) JsClock.prototype[Symbol.dispose] = JsClock.prototype.free;

/**
 * Trusted JS context-provider wrapper.
 */
export class JsContextProvider {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        JsContextProviderFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_jscontextprovider_free(ptr, 0);
    }
    /**
     * Construct a context-provider wrapper around a trusted host adapter.
     *
     * # Errors
     *
     * Returns a TypeError-equivalent when options or the adapter are invalid.
     * @param {any} adapter
     * @param {any} options
     */
    constructor(adapter, options) {
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.jscontextprovider_new(retptr, addHeapObject(adapter), addHeapObject(options));
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            var r2 = getDataViewMemory0().getInt32(retptr + 4 * 2, true);
            if (r2) {
                throw takeObject(r1);
            }
            this.__wbg_ptr = r0;
            JsContextProviderFinalization.register(this, this.__wbg_ptr, this);
            return this;
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
        }
    }
}
if (Symbol.dispose) JsContextProvider.prototype[Symbol.dispose] = JsContextProvider.prototype.free;

/**
 * Trusted JS journal-store wrapper. Not crash-durable.
 */
export class JsJournalStore {
    static __wrap(ptr) {
        const obj = Object.create(JsJournalStore.prototype);
        obj.__wbg_ptr = ptr;
        JsJournalStoreFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        JsJournalStoreFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_jsjournalstore_free(ptr, 0);
    }
    /**
     * Clone the wrapper without moving the caller's handle.
     * @returns {JsJournalStore}
     */
    cloneHandle() {
        const ret = wasm.jsjournalstore_cloneHandle(this.__wbg_ptr);
        return JsJournalStore.__wrap(ret);
    }
    /**
     * Construct a journal-store wrapper around a trusted host adapter.
     *
     * # Errors
     *
     * Returns a TypeError-equivalent when the adapter is invalid.
     * @param {any} adapter
     * @param {any} options
     */
    constructor(adapter, options) {
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.jsjournalstore_new(retptr, addHeapObject(adapter), addHeapObject(options));
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            var r2 = getDataViewMemory0().getInt32(retptr + 4 * 2, true);
            if (r2) {
                throw takeObject(r1);
            }
            this.__wbg_ptr = r0;
            JsJournalStoreFinalization.register(this, this.__wbg_ptr, this);
            return this;
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
        }
    }
}
if (Symbol.dispose) JsJournalStore.prototype[Symbol.dispose] = JsJournalStore.prototype.free;

/**
 * Trusted JS middleware wrapper.
 */
export class JsMiddleware {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        JsMiddlewareFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_jsmiddleware_free(ptr, 0);
    }
    /**
     * Construct a middleware wrapper around a trusted host adapter.
     *
     * # Errors
     *
     * Returns a TypeError-equivalent when options or the adapter are invalid.
     * @param {any} adapter
     * @param {any} options
     */
    constructor(adapter, options) {
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.jsmiddleware_new(retptr, addHeapObject(adapter), addHeapObject(options));
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            var r2 = getDataViewMemory0().getInt32(retptr + 4 * 2, true);
            if (r2) {
                throw takeObject(r1);
            }
            this.__wbg_ptr = r0;
            JsMiddlewareFinalization.register(this, this.__wbg_ptr, this);
            return this;
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
        }
    }
}
if (Symbol.dispose) JsMiddleware.prototype[Symbol.dispose] = JsMiddleware.prototype.free;

/**
 * Trusted JS model wrapper. Not an Agent handle.
 */
export class JsModel {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        JsModelFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_jsmodel_free(ptr, 0);
    }
    /**
     * Construct a model wrapper around a trusted host adapter.
     *
     * # Errors
     *
     * Returns a TypeError-equivalent when options or the adapter are invalid.
     * @param {any} adapter
     * @param {any} options
     */
    constructor(adapter, options) {
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.jsmodel_new(retptr, addHeapObject(adapter), addHeapObject(options));
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            var r2 = getDataViewMemory0().getInt32(retptr + 4 * 2, true);
            if (r2) {
                throw takeObject(r1);
            }
            this.__wbg_ptr = r0;
            JsModelFinalization.register(this, this.__wbg_ptr, this);
            return this;
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
        }
    }
}
if (Symbol.dispose) JsModel.prototype[Symbol.dispose] = JsModel.prototype.free;

/**
 * Trusted JS observer wrapper.
 */
export class JsObserver {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        JsObserverFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_jsobserver_free(ptr, 0);
    }
    /**
     * Construct an observer wrapper around a trusted host adapter.
     *
     * # Errors
     *
     * Returns a TypeError-equivalent when options or the adapter are invalid.
     * @param {any} adapter
     * @param {any} options
     */
    constructor(adapter, options) {
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.jsobserver_new(retptr, addHeapObject(adapter), addHeapObject(options));
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            var r2 = getDataViewMemory0().getInt32(retptr + 4 * 2, true);
            if (r2) {
                throw takeObject(r1);
            }
            this.__wbg_ptr = r0;
            JsObserverFinalization.register(this, this.__wbg_ptr, this);
            return this;
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
        }
    }
}
if (Symbol.dispose) JsObserver.prototype[Symbol.dispose] = JsObserver.prototype.free;

/**
 * Host random-source wrapper.
 */
export class JsRandomSource {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        JsRandomSourceFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_jsrandomsource_free(ptr, 0);
    }
    /**
     * Construct a random-source wrapper around a trusted host adapter.
     *
     * # Errors
     *
     * Returns a TypeError-equivalent when `fillBytes` is missing.
     * @param {any} adapter
     */
    constructor(adapter) {
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.jsrandomsource_new(retptr, addHeapObject(adapter));
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            var r2 = getDataViewMemory0().getInt32(retptr + 4 * 2, true);
            if (r2) {
                throw takeObject(r1);
            }
            this.__wbg_ptr = r0;
            JsRandomSourceFinalization.register(this, this.__wbg_ptr, this);
            return this;
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
        }
    }
}
if (Symbol.dispose) JsRandomSource.prototype[Symbol.dispose] = JsRandomSource.prototype.free;

/**
 * Trusted JS toolset wrapper. Not an Agent handle.
 */
export class JsToolset {
    static __wrap(ptr) {
        const obj = Object.create(JsToolset.prototype);
        obj.__wbg_ptr = ptr;
        JsToolsetFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    static __unwrap(jsValue) {
        if (!(jsValue instanceof JsToolset)) {
            return 0;
        }
        return jsValue.__destroy_into_raw();
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        JsToolsetFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_jstoolset_free(ptr, 0);
    }
    /**
     * Clone the wrapper without moving the caller's handle.
     * @returns {JsToolset}
     */
    cloneHandle() {
        const ret = wasm.jstoolset_cloneHandle(this.__wbg_ptr);
        return JsToolset.__wrap(ret);
    }
    /**
     * Construct a toolset wrapper around a trusted host adapter.
     *
     * # Errors
     *
     * Returns a TypeError-equivalent when options or the adapter are invalid.
     * @param {any} adapter
     * @param {any} options
     */
    constructor(adapter, options) {
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.jstoolset_new(retptr, addHeapObject(adapter), addHeapObject(options));
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            var r2 = getDataViewMemory0().getInt32(retptr + 4 * 2, true);
            if (r2) {
                throw takeObject(r1);
            }
            this.__wbg_ptr = r0;
            JsToolsetFinalization.register(this, this.__wbg_ptr, this);
            return this;
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
        }
    }
}
if (Symbol.dispose) JsToolset.prototype[Symbol.dispose] = JsToolset.prototype.free;

/**
 * Live lane handle.
 */
export class Lane {
    static __wrap(ptr) {
        const obj = Object.create(Lane.prototype);
        obj.__wbg_ptr = ptr;
        LaneFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        LaneFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_lane_free(ptr, 0);
    }
    /**
     * Inspect name, leaf, and history length.
     *
     * # Errors
     *
     * Returns a structured host error when the lane cannot be inspected.
     * @returns {Promise<any>}
     */
    inspect() {
        const ret = wasm.lane_inspect(this.__wbg_ptr);
        return takeObject(ret);
    }
    /**
     * Durable lane identity.
     * @returns {string}
     */
    get laneId() {
        let deferred1_0;
        let deferred1_1;
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.lane_laneId(retptr, this.__wbg_ptr);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            deferred1_0 = r0;
            deferred1_1 = r1;
            return getStringFromWasm0(r0, r1);
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
            wasm.__wbindgen_export5(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Point this idle lane at an existing entry without copying.
     *
     * # Errors
     *
     * Returns a structured host error when the entry is unknown or the lane
     * is busy.
     * @param {string} entry_id
     * @returns {Promise<any>}
     */
    navigate(entry_id) {
        const ptr0 = passStringToWasm0(entry_id, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.lane_navigate(this.__wbg_ptr, ptr0, len0);
        return takeObject(ret);
    }
    /**
     * Session that owns this lane.
     * @returns {Session}
     */
    get session() {
        const ret = wasm.lane_session(this.__wbg_ptr);
        return Session.__wrap(ret);
    }
}
if (Symbol.dispose) Lane.prototype[Symbol.dispose] = Lane.prototype.free;

/**
 * Read-only operation locator.
 */
export class Locator {
    static __wrap(ptr) {
        const obj = Object.create(Locator.prototype);
        obj.__wbg_ptr = ptr;
        LocatorFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        LocatorFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_locator_free(ptr, 0);
    }
    /**
     * Lane identity.
     * @returns {string}
     */
    get laneId() {
        let deferred1_0;
        let deferred1_1;
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.locator_laneId(retptr, this.__wbg_ptr);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            deferred1_0 = r0;
            deferred1_1 = r1;
            return getStringFromWasm0(r0, r1);
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
            wasm.__wbindgen_export5(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Run identity.
     * @returns {string}
     */
    get runId() {
        let deferred1_0;
        let deferred1_1;
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.locator_runId(retptr, this.__wbg_ptr);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            deferred1_0 = r0;
            deferred1_1 = r1;
            return getStringFromWasm0(r0, r1);
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
            wasm.__wbindgen_export5(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Session identity.
     * @returns {string}
     */
    get sessionId() {
        let deferred1_0;
        let deferred1_1;
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.locator_sessionId(retptr, this.__wbg_ptr);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            deferred1_0 = r0;
            deferred1_1 = r1;
            return getStringFromWasm0(r0, r1);
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
            wasm.__wbindgen_export5(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Tenant scope captured at acceptance.
     * @returns {string}
     */
    get tenantScope() {
        let deferred1_0;
        let deferred1_1;
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.locator_tenantScope(retptr, this.__wbg_ptr);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            deferred1_0 = r0;
            deferred1_1 = r1;
            return getStringFromWasm0(r0, r1);
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
            wasm.__wbindgen_export5(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Explicit locator snapshot.
     *
     * # Errors
     *
     * Returns a JavaScript exception when the snapshot object cannot be constructed.
     * @returns {any}
     */
    toDict() {
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.locator_toDict(retptr, this.__wbg_ptr);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            var r2 = getDataViewMemory0().getInt32(retptr + 4 * 2, true);
            if (r2) {
                throw takeObject(r1);
            }
            return takeObject(r0);
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
        }
    }
}
if (Symbol.dispose) Locator.prototype[Symbol.dispose] = Locator.prototype.free;

/**
 * In-process external identity map.
 */
export class MemoryExternalIdentityMap {
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        MemoryExternalIdentityMapFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_memoryexternalidentitymap_free(ptr, 0);
    }
    /**
     * Empty map.
     */
    constructor() {
        const ret = wasm.memoryexternalidentitymap_new();
        this.__wbg_ptr = ret;
        MemoryExternalIdentityMapFinalization.register(this, this.__wbg_ptr, this);
        return this;
    }
    /**
     * Resolve one previously bound key.
     *
     * # Errors
     *
     * Returns a structured host error when the key is invalid.
     * @param {string} channel
     * @param {string} account
     * @param {string} thread
     * @returns {any}
     */
    resolve(channel, account, thread) {
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            const ptr0 = passStringToWasm0(channel, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            const len0 = WASM_VECTOR_LEN;
            const ptr1 = passStringToWasm0(account, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            const len1 = WASM_VECTOR_LEN;
            const ptr2 = passStringToWasm0(thread, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            const len2 = WASM_VECTOR_LEN;
            wasm.memoryexternalidentitymap_resolve(retptr, this.__wbg_ptr, ptr0, len0, ptr1, len1, ptr2, len2);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            var r2 = getDataViewMemory0().getInt32(retptr + 4 * 2, true);
            if (r2) {
                throw takeObject(r1);
            }
            return takeObject(r0);
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
        }
    }
}
if (Symbol.dispose) MemoryExternalIdentityMap.prototype[Symbol.dispose] = MemoryExternalIdentityMap.prototype.free;

/**
 * Detached run control handle. Drop detaches observation and does not cancel.
 */
export class Run {
    static __wrap(ptr) {
        const obj = Object.create(Run.prototype);
        obj.__wbg_ptr = ptr;
        RunFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        RunFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_run_free(ptr, 0);
    }
    /**
     * Submit idempotent durable cancellation.
     *
     * # Errors
     *
     * Returns a structured host error when cancellation cannot be committed.
     * @param {string | null} [_reason]
     * @returns {Promise<any>}
     */
    cancel(_reason) {
        var ptr0 = isLikeNone(_reason) ? 0 : passStringToWasm0(_reason, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        var len0 = WASM_VECTOR_LEN;
        const ret = wasm.run_cancel(this.__wbg_ptr, ptr0, len0);
        return takeObject(ret);
    }
    /**
     * Close event delivery without cancelling the run.
     * @returns {Promise<any>}
     */
    closeEvents() {
        const ret = wasm.run_closeEvents(this.__wbg_ptr);
        return takeObject(ret);
    }
    /**
     * Immutable operation locator snapshot.
     * @returns {Locator}
     */
    get locator() {
        const ret = wasm.run_locator(this.__wbg_ptr);
        return Locator.__wrap(ret);
    }
    /**
     * Receive the next transport batch, or `undefined` after close/terminal.
     *
     * # Errors
     *
     * Returns a structured host error when event delivery fails to start.
     * @returns {Promise<any>}
     */
    nextEventBatch() {
        const ret = wasm.run_nextEventBatch(this.__wbg_ptr);
        return takeObject(ret);
    }
    /**
     * Wait for the retained terminal result.
     *
     * # Errors
     *
     * Returns a structured host error when the run fails, times out, or is cancelled.
     * @returns {Promise<any>}
     */
    result() {
        const ret = wasm.run_result(this.__wbg_ptr);
        return takeObject(ret);
    }
    /**
     * Live session handle for this run.
     * @returns {Session}
     */
    get session() {
        const ret = wasm.run_session(this.__wbg_ptr);
        return Session.__wrap(ret);
    }
}
if (Symbol.dispose) Run.prototype[Symbol.dispose] = Run.prototype.free;

/**
 * Successful terminal result handle.
 */
export class RunResult {
    static __wrap(ptr) {
        const obj = Object.create(RunResult.prototype);
        obj.__wbg_ptr = ptr;
        RunResultFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        RunResultFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_runresult_free(ptr, 0);
    }
    /**
     * Complete Rust-owned capability activation set for this run.
     *
     * # Errors
     *
     * Returns a JavaScript exception when the activation objects cannot be constructed.
     * @returns {any}
     */
    get activeCapabilities() {
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.runresult_activeCapabilities(retptr, this.__wbg_ptr);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            var r2 = getDataViewMemory0().getInt32(retptr + 4 * 2, true);
            if (r2) {
                throw takeObject(r1);
            }
            return takeObject(r0);
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
        }
    }
    /**
     * Operation locator for the completed run.
     * @returns {Locator}
     */
    get locator() {
        const ret = wasm.runresult_locator(this.__wbg_ptr);
        return Locator.__wrap(ret);
    }
    /**
     * Durable retry attempts consumed by this run.
     * @returns {number}
     */
    get retryAttempts() {
        const ret = wasm.runresult_retryAttempts(this.__wbg_ptr);
        return ret >>> 0;
    }
    /**
     * Locator snapshot for the completed run.
     * @returns {Locator}
     */
    get session() {
        const ret = wasm.runresult_session(this.__wbg_ptr);
        return Locator.__wrap(ret);
    }
    /**
     * Concatenated final assistant text.
     * @returns {string}
     */
    get text() {
        let deferred1_0;
        let deferred1_1;
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.runresult_text(retptr, this.__wbg_ptr);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            deferred1_0 = r0;
            deferred1_1 = r1;
            return getStringFromWasm0(r0, r1);
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
            wasm.__wbindgen_export5(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Explicit result snapshot.
     *
     * # Errors
     *
     * Returns a JavaScript exception when the snapshot object cannot be constructed.
     * @returns {any}
     */
    toDict() {
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.runresult_toDict(retptr, this.__wbg_ptr);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            var r2 = getDataViewMemory0().getInt32(retptr + 4 * 2, true);
            if (r2) {
                throw takeObject(r1);
            }
            return takeObject(r0);
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
        }
    }
    /**
     * Stable Rust-owned committed record-kind trace in journal order.
     * @returns {string[]}
     */
    get trace() {
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.runresult_trace(retptr, this.__wbg_ptr);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            var v1 = getArrayJsValueFromWasm0(r0, r1).slice();
            wasm.__wbindgen_export5(r0, r1 * 4, 4);
            return v1;
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
        }
    }
}
if (Symbol.dispose) RunResult.prototype[Symbol.dispose] = RunResult.prototype.free;

/**
 * Live session handle.
 */
export class Session {
    static __wrap(ptr) {
        const obj = Object.create(Session.prototype);
        obj.__wbg_ptr = ptr;
        SessionFinalization.register(obj, obj.__wbg_ptr, obj);
        return obj;
    }
    __destroy_into_raw() {
        const ptr = this.__wbg_ptr;
        this.__wbg_ptr = 0;
        SessionFinalization.unregister(this);
        return ptr;
    }
    free() {
        const ptr = this.__destroy_into_raw();
        wasm.__wbg_session_free(ptr, 0);
    }
    /**
     * Bind a host-owned external identity to one lane.
     *
     * # Errors
     *
     * Returns a structured host error when the key is invalid, the lane is
     * unknown, or the key is already bound to a different session lane.
     * @param {MemoryExternalIdentityMap} map
     * @param {string} channel
     * @param {string} account
     * @param {string} thread
     * @param {string} lane_id
     */
    bindExternalIdentity(map, channel, account, thread, lane_id) {
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            _assertClass(map, MemoryExternalIdentityMap);
            const ptr0 = passStringToWasm0(channel, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            const len0 = WASM_VECTOR_LEN;
            const ptr1 = passStringToWasm0(account, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            const len1 = WASM_VECTOR_LEN;
            const ptr2 = passStringToWasm0(thread, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            const len2 = WASM_VECTOR_LEN;
            const ptr3 = passStringToWasm0(lane_id, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            const len3 = WASM_VECTOR_LEN;
            wasm.session_bindExternalIdentity(retptr, this.__wbg_ptr, map.__wbg_ptr, ptr0, len0, ptr1, len1, ptr2, len2, ptr3, len3);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            if (r1) {
                throw takeObject(r0);
            }
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
        }
    }
    /**
     * Create a named lane, optionally forking from an existing entry.
     *
     * # Errors
     *
     * Returns a structured host error when the lane cannot be created.
     * @param {string} name
     * @param {string | null} [fork]
     * @returns {Promise<any>}
     */
    createLane(name, fork) {
        const ptr0 = passStringToWasm0(name, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len0 = WASM_VECTOR_LEN;
        var ptr1 = isLikeNone(fork) ? 0 : passStringToWasm0(fork, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        var len1 = WASM_VECTOR_LEN;
        const ret = wasm.session_createLane(this.__wbg_ptr, ptr0, len0, ptr1, len1);
        return takeObject(ret);
    }
    /**
     * Look up one lane by application name.
     *
     * # Errors
     *
     * Returns a structured host error when the lane does not exist.
     * @param {string} name
     * @returns {Promise<any>}
     */
    lane(name) {
        const ptr0 = passStringToWasm0(name, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len0 = WASM_VECTOR_LEN;
        const ret = wasm.session_lane(this.__wbg_ptr, ptr0, len0);
        return takeObject(ret);
    }
    /**
     * List restored lanes.
     *
     * # Errors
     *
     * Returns a structured host error when the session cannot be loaded.
     * @returns {Promise<any>}
     */
    listLanes() {
        const ret = wasm.session_listLanes(this.__wbg_ptr);
        return takeObject(ret);
    }
    /**
     * Session identity.
     * @returns {string}
     */
    get sessionId() {
        let deferred1_0;
        let deferred1_1;
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.session_sessionId(retptr, this.__wbg_ptr);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            deferred1_0 = r0;
            deferred1_1 = r1;
            return getStringFromWasm0(r0, r1);
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
            wasm.__wbindgen_export5(deferred1_0, deferred1_1, 1);
        }
    }
    /**
     * Tenant scope captured by the host.
     * @returns {string}
     */
    get tenantScope() {
        let deferred1_0;
        let deferred1_1;
        try {
            const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
            wasm.session_tenantScope(retptr, this.__wbg_ptr);
            var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
            var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
            deferred1_0 = r0;
            deferred1_1 = r1;
            return getStringFromWasm0(r0, r1);
        } finally {
            wasm.__wbindgen_add_to_stack_pointer(16);
            wasm.__wbindgen_export5(deferred1_0, deferred1_1, 1);
        }
    }
}
if (Symbol.dispose) Session.prototype[Symbol.dispose] = Session.prototype.free;

/**
 * Apply normalized coordinator commands and return identity traces.
 *
 * This export is test-only and does not submit a live Agent.
 *
 * # Errors
 *
 * Returns a TypeError-equivalent when any command fails DTO validation.
 * @param {string} encoded
 * @returns {string}
 */
export function applyScriptedCoordinatorCommands(encoded) {
    let deferred3_0;
    let deferred3_1;
    try {
        const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
        const ptr0 = passStringToWasm0(encoded, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len0 = WASM_VECTOR_LEN;
        wasm.applyScriptedCoordinatorCommands(retptr, ptr0, len0);
        var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
        var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
        var r2 = getDataViewMemory0().getInt32(retptr + 4 * 2, true);
        var r3 = getDataViewMemory0().getInt32(retptr + 4 * 3, true);
        var ptr2 = r0;
        var len2 = r1;
        if (r3) {
            ptr2 = 0; len2 = 0;
            throw takeObject(r2);
        }
        deferred3_0 = ptr2;
        deferred3_1 = len2;
        return getStringFromWasm0(ptr2, len2);
    } finally {
        wasm.__wbindgen_add_to_stack_pointer(16);
        wasm.__wbindgen_export5(deferred3_0, deferred3_1, 1);
    }
}

/**
 * Lockstep version metadata for the wasm package.
 *
 * # Errors
 *
 * Returns a JavaScript exception when the metadata object cannot be constructed.
 * @returns {any}
 */
export function buildMetadata() {
    try {
        const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
        wasm.buildMetadata(retptr);
        var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
        var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
        var r2 = getDataViewMemory0().getInt32(retptr + 4 * 2, true);
        if (r2) {
            throw takeObject(r1);
        }
        return takeObject(r0);
    } finally {
        wasm.__wbindgen_add_to_stack_pointer(16);
    }
}

/**
 * Construct the six-port compile fixtures for the current target.
 */
export function compilePortProxies() {
    wasm.compilePortProxies();
}

/**
 * Drive scripted journal-store health. Test-only; always non-durable.
 *
 * # Errors
 *
 * Returns a JavaScript exception when the adapter is invalid.
 * @param {any} adapter
 * @param {any} options
 * @returns {Promise<any>}
 */
export function driveScriptedJournalHealth(adapter, options) {
    const ret = wasm.driveScriptedJournalHealth(addHeapObject(adapter), addHeapObject(options));
    return takeObject(ret);
}

/**
 * Drive one scripted `Model::request`. Test-only; not an Agent run.
 *
 * # Errors
 *
 * Returns a JavaScript exception when options cannot be parsed.
 * @param {any} adapter
 * @param {any} options
 * @param {any} signal
 * @returns {Promise<any>}
 */
export function driveScriptedModelRequest(adapter, options, signal) {
    const ret = wasm.driveScriptedModelRequest(addHeapObject(adapter), addHeapObject(options), addHeapObject(signal));
    return takeObject(ret);
}

/**
 * Drive one scripted `Toolset::call`. Test-only; not an Agent run.
 *
 * # Errors
 *
 * Returns a JavaScript exception when options cannot be parsed.
 * @param {any} adapter
 * @param {any} options
 * @param {any} signal
 * @returns {Promise<any>}
 */
export function driveScriptedToolCall(adapter, options, signal) {
    const ret = wasm.driveScriptedToolCall(addHeapObject(adapter), addHeapObject(options), addHeapObject(signal));
    return takeObject(ret);
}

/**
 * Process-local health token. Does not create a runtime, open a store, or spawn work.
 * @returns {string}
 */
export function health() {
    let deferred1_0;
    let deferred1_1;
    try {
        const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
        wasm.health(retptr);
        var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
        var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
        deferred1_0 = r0;
        deferred1_1 = r1;
        return getStringFromWasm0(r0, r1);
    } finally {
        wasm.__wbindgen_add_to_stack_pointer(16);
        wasm.__wbindgen_export5(deferred1_0, deferred1_1, 1);
    }
}

/**
 * Compute journal known-answer hex through the one Rust engine.
 *
 * # Errors
 *
 * Returns a TypeError-equivalent when `kind` or the diagnostic JSON is invalid.
 * @param {string} kind
 * @param {string} encoded
 * @returns {string}
 */
export function journalKnownAnswer(kind, encoded) {
    let deferred4_0;
    let deferred4_1;
    try {
        const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
        const ptr0 = passStringToWasm0(kind, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(encoded, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len1 = WASM_VECTOR_LEN;
        wasm.journalKnownAnswer(retptr, ptr0, len0, ptr1, len1);
        var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
        var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
        var r2 = getDataViewMemory0().getInt32(retptr + 4 * 2, true);
        var r3 = getDataViewMemory0().getInt32(retptr + 4 * 3, true);
        var ptr3 = r0;
        var len3 = r1;
        if (r3) {
            ptr3 = 0; len3 = 0;
            throw takeObject(r2);
        }
        deferred4_0 = ptr3;
        deferred4_1 = len3;
        return getStringFromWasm0(ptr3, len3);
    } finally {
        wasm.__wbindgen_add_to_stack_pointer(16);
        wasm.__wbindgen_export5(deferred4_0, deferred4_1, 1);
    }
}

/**
 * Normalize a pre-beta lineage or authenticated external-command shape.
 *
 * # Errors
 *
 * Returns a TypeError-equivalent when `kind` is unsupported or `value` is invalid.
 * @param {string} kind
 * @param {string} encoded
 * @returns {string}
 */
export function normalizePrebetaShape(kind, encoded) {
    let deferred4_0;
    let deferred4_1;
    try {
        const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
        const ptr0 = passStringToWasm0(kind, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len0 = WASM_VECTOR_LEN;
        const ptr1 = passStringToWasm0(encoded, wasm.__wbindgen_export, wasm.__wbindgen_export2);
        const len1 = WASM_VECTOR_LEN;
        wasm.normalizePrebetaShape(retptr, ptr0, len0, ptr1, len1);
        var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
        var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
        var r2 = getDataViewMemory0().getInt32(retptr + 4 * 2, true);
        var r3 = getDataViewMemory0().getInt32(retptr + 4 * 3, true);
        var ptr3 = r0;
        var len3 = r1;
        if (r3) {
            ptr3 = 0; len3 = 0;
            throw takeObject(r2);
        }
        deferred4_0 = ptr3;
        deferred4_1 = len3;
        return getStringFromWasm0(ptr3, len3);
    } finally {
        wasm.__wbindgen_add_to_stack_pointer(16);
        wasm.__wbindgen_export5(deferred4_0, deferred4_1, 1);
    }
}

/**
 * Run the embedded no-op golden trace through `CommitCoordinator`.
 *
 * This export is a test-only engine-load proof and is not part of the public
 * TypeScript surface.
 *
 * # Errors
 *
 * Returns a JavaScript exception when the fixture, store, or report fails.
 * @returns {string}
 */
export function runNoopTrace() {
    let deferred2_0;
    let deferred2_1;
    try {
        const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
        wasm.runNoopTrace(retptr);
        var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
        var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
        var r2 = getDataViewMemory0().getInt32(retptr + 4 * 2, true);
        var r3 = getDataViewMemory0().getInt32(retptr + 4 * 3, true);
        var ptr1 = r0;
        var len1 = r1;
        if (r3) {
            ptr1 = 0; len1 = 0;
            throw takeObject(r2);
        }
        deferred2_0 = ptr1;
        deferred2_1 = len1;
        return getStringFromWasm0(ptr1, len1);
    } finally {
        wasm.__wbindgen_add_to_stack_pointer(16);
        wasm.__wbindgen_export5(deferred2_0, deferred2_1, 1);
    }
}

/**
 * Install the host driver when the generated module loads.
 */
export function wasm_start() {
    wasm.wasm_start();
}
function __wbg_get_imports() {
    const import0 = {
        __proto__: null,
        __wbg___wbindgen_boolean_get_c9c83ebd41b34df3: function(arg0) {
            const v = getObject(arg0);
            const ret = typeof(v) === 'boolean' ? v : undefined;
            return isLikeNone(ret) ? 0xFFFFFF : ret ? 1 : 0;
        },
        __wbg___wbindgen_is_function_5e4570eb24ffa122: function(arg0) {
            const ret = typeof(getObject(arg0)) === 'function';
            return ret;
        },
        __wbg___wbindgen_is_null_7d13f41e1a2d5140: function(arg0) {
            const ret = getObject(arg0) === null;
            return ret;
        },
        __wbg___wbindgen_is_undefined_6cff064c44e0d823: function(arg0) {
            const ret = getObject(arg0) === undefined;
            return ret;
        },
        __wbg___wbindgen_number_get_136b9679cab35cfb: function(arg0, arg1) {
            const obj = getObject(arg1);
            const ret = typeof(obj) === 'number' ? obj : undefined;
            getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
        },
        __wbg___wbindgen_string_get_d154f1e671052120: function(arg0, arg1) {
            const obj = getObject(arg1);
            const ret = typeof(obj) === 'string' ? obj : undefined;
            var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_export, wasm.__wbindgen_export2);
            var len1 = WASM_VECTOR_LEN;
            getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
            getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        },
        __wbg___wbindgen_throw_bb96b2010945f0bc: function(arg0, arg1) {
            throw new Error(getStringFromWasm0(arg0, arg1));
        },
        __wbg__wbg_cb_unref_be22cc64ae6946a0: function(arg0) {
            getObject(arg0)._wbg_cb_unref();
        },
        __wbg_agent_new: function(arg0) {
            const ret = Agent.__wrap(arg0);
            return addHeapObject(ret);
        },
        __wbg_apply_cb180996ed7fdae9: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = getObject(arg0).apply(getObject(arg1), getObject(arg2));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_asyncIterator_6a2c651f755e0440: function() {
            const ret = Symbol.asyncIterator;
            return addHeapObject(ret);
        },
        __wbg_call_0f2a9af232c18fd2: function() { return handleError(function (arg0, arg1, arg2, arg3) {
            const ret = getObject(arg0).call(getObject(arg1), getObject(arg2), getObject(arg3));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_call_1c5886ab9c57d1c7: function() { return handleError(function (arg0, arg1) {
            const ret = getObject(arg0).call(getObject(arg1));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_call_35dba3c747ad7521: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = getObject(arg0).call(getObject(arg1), getObject(arg2));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_construct_a5c4a12c650f2c30: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.construct(getObject(arg0), getObject(arg1));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_event_new: function(arg0) {
            const ret = Event.__wrap(arg0);
            return addHeapObject(ret);
        },
        __wbg_eventbatch_new: function(arg0) {
            const ret = EventBatch.__wrap(arg0);
            return addHeapObject(ret);
        },
        __wbg_getRandomValues_a608c4436c19407a: function() { return handleError(function (arg0, arg1) {
            globalThis.crypto.getRandomValues(getArrayU8FromWasm0(arg0, arg1));
        }, arguments); },
        __wbg_get_971a0c45d172643f: function() { return handleError(function (arg0, arg1) {
            const ret = Reflect.get(getObject(arg0), getObject(arg1));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_instanceof_Promise_e6e764b945c3128a: function(arg0) {
            let result;
            try {
                result = getObject(arg0) instanceof Promise;
            } catch (_) {
                result = false;
            }
            const ret = result;
            return ret;
        },
        __wbg_jstoolset_unwrap: function(arg0) {
            const ret = JsToolset.__unwrap(getObject(arg0));
            return ret;
        },
        __wbg_lane_new: function(arg0) {
            const ret = Lane.__wrap(arg0);
            return addHeapObject(ret);
        },
        __wbg_length_36bd29c6848c2144: function(arg0) {
            const ret = getObject(arg0).length;
            return ret;
        },
        __wbg_new_116be93542d39019: function() {
            const ret = new Array();
            return addHeapObject(ret);
        },
        __wbg_new_15ae2532051588db: function(arg0, arg1) {
            const ret = new TypeError(getStringFromWasm0(arg0, arg1));
            return addHeapObject(ret);
        },
        __wbg_new_358857d90afd5a2d: function(arg0, arg1) {
            const ret = new Error(getStringFromWasm0(arg0, arg1));
            return addHeapObject(ret);
        },
        __wbg_new_ebe3e0f6837f0879: function() {
            const ret = new Object();
            return addHeapObject(ret);
        },
        __wbg_new_from_slice_3eea173078478cfe: function(arg0, arg1) {
            const ret = new Uint8Array(getArrayU8FromWasm0(arg0, arg1));
            return addHeapObject(ret);
        },
        __wbg_new_no_args_51d9cfd7bb73258c: function(arg0, arg1) {
            const ret = new Function(getStringFromWasm0(arg0, arg1));
            return addHeapObject(ret);
        },
        __wbg_new_typed_cceaf62d8d95e9f2: function(arg0, arg1) {
            try {
                var state0 = {a: arg0, b: arg1};
                var cb0 = (arg0, arg1) => {
                    const a = state0.a;
                    state0.a = 0;
                    try {
                        return __wasm_bindgen_func_elem_1531(a, state0.b, arg0, arg1);
                    } finally {
                        state0.a = a;
                    }
                };
                const ret = new Promise(cb0);
                return addHeapObject(ret);
            } finally {
                state0.a = 0;
            }
        },
        __wbg_new_with_length_3ffc1c56427c525c: function(arg0) {
            const ret = new Uint8Array(arg0 >>> 0);
            return addHeapObject(ret);
        },
        __wbg_now_8b265300afd5f2b9: function() {
            const ret = Date.now();
            return ret;
        },
        __wbg_prototypesetcall_de8e0d9553586985: function(arg0, arg1, arg2) {
            Uint8Array.prototype.set.call(getArrayU8FromWasm0(arg0, arg1), getObject(arg2));
        },
        __wbg_push_adb0107829f02d75: function(arg0, arg1) {
            const ret = getObject(arg0).push(getObject(arg1));
            return ret;
        },
        __wbg_queueMicrotask_ac694eae12e92dfb: function(arg0) {
            queueMicrotask(getObject(arg0));
        },
        __wbg_queueMicrotask_be5fe34a8f4cad4d: function(arg0) {
            const ret = getObject(arg0).queueMicrotask;
            return addHeapObject(ret);
        },
        __wbg_reject_671a1c459689d0e0: function(arg0) {
            const ret = Promise.reject(getObject(arg0));
            return addHeapObject(ret);
        },
        __wbg_resolve_020f95d838c6ef25: function(arg0) {
            const ret = Promise.resolve(getObject(arg0));
            return addHeapObject(ret);
        },
        __wbg_runresult_new: function(arg0) {
            const ret = RunResult.__wrap(arg0);
            return addHeapObject(ret);
        },
        __wbg_session_new: function(arg0) {
            const ret = Session.__wrap(arg0);
            return addHeapObject(ret);
        },
        __wbg_set_8155bb79a948541b: function() { return handleError(function (arg0, arg1, arg2) {
            const ret = Reflect.set(getObject(arg0), getObject(arg1), getObject(arg2));
            return ret;
        }, arguments); },
        __wbg_static_accessor_GLOBAL_THIS_466428f93b4eaa76: function() {
            const ret = typeof globalThis === 'undefined' ? null : globalThis;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        },
        __wbg_static_accessor_GLOBAL_c7aea38d4de089bc: function() {
            const ret = typeof global === 'undefined' ? null : global;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        },
        __wbg_static_accessor_SELF_42d4fae05e59267a: function() {
            const ret = typeof self === 'undefined' ? null : self;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        },
        __wbg_static_accessor_WINDOW_e0db14a0eba6a812: function() {
            const ret = typeof window === 'undefined' ? null : window;
            return isLikeNone(ret) ? 0 : addHeapObject(ret);
        },
        __wbg_stringify_f93a4ebae9231922: function() { return handleError(function (arg0) {
            const ret = JSON.stringify(getObject(arg0));
            return addHeapObject(ret);
        }, arguments); },
        __wbg_then_7026b513a94278a8: function(arg0, arg1) {
            const ret = getObject(arg0).then(getObject(arg1));
            return addHeapObject(ret);
        },
        __wbg_then_72819b8d4e081fb5: function(arg0, arg1, arg2) {
            const ret = getObject(arg0).then(getObject(arg1), getObject(arg2));
            return addHeapObject(ret);
        },
        __wbindgen_cast_0000000000000001: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [Externref], shim_idx: 484, ret: Result(Unit), inner_ret: Some(Result(Unit)) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, __wasm_bindgen_func_elem_1517);
            return addHeapObject(ret);
        },
        __wbindgen_cast_0000000000000002: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [], shim_idx: 5, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, __wasm_bindgen_func_elem_375);
            return addHeapObject(ret);
        },
        __wbindgen_cast_0000000000000003: function(arg0) {
            // Cast intrinsic for `F64 -> Externref`.
            const ret = arg0;
            return addHeapObject(ret);
        },
        __wbindgen_cast_0000000000000004: function(arg0, arg1) {
            // Cast intrinsic for `Ref(String) -> Externref`.
            const ret = getStringFromWasm0(arg0, arg1);
            return addHeapObject(ret);
        },
        __wbindgen_cast_0000000000000005: function(arg0) {
            // Cast intrinsic for `U64 -> Externref`.
            const ret = BigInt.asUintN(64, arg0);
            return addHeapObject(ret);
        },
        __wbindgen_object_clone_ref: function(arg0) {
            const ret = getObject(arg0);
            return addHeapObject(ret);
        },
        __wbindgen_object_drop_ref: function(arg0) {
            takeObject(arg0);
        },
    };
    return {
        __proto__: null,
        "./finstack_ai_wasm_bg.js": import0,
    };
}

function __wasm_bindgen_func_elem_375(arg0, arg1) {
    wasm.__wasm_bindgen_func_elem_375(arg0, arg1);
}

function __wasm_bindgen_func_elem_1517(arg0, arg1, arg2) {
    try {
        const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
        wasm.__wasm_bindgen_func_elem_1517(retptr, arg0, arg1, addHeapObject(arg2));
        var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
        var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
        if (r1) {
            throw takeObject(r0);
        }
    } finally {
        wasm.__wbindgen_add_to_stack_pointer(16);
    }
}

function __wasm_bindgen_func_elem_1531(arg0, arg1, arg2, arg3) {
    wasm.__wasm_bindgen_func_elem_1531(arg0, arg1, addHeapObject(arg2), addHeapObject(arg3));
}

const AgentFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_agent_free(ptr, 1));
const EventFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_event_free(ptr, 1));
const EventBatchFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_eventbatch_free(ptr, 1));
const JsArtifactStoreFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_jsartifactstore_free(ptr, 1));
const JsClockFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_jsclock_free(ptr, 1));
const JsContextProviderFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_jscontextprovider_free(ptr, 1));
const JsJournalStoreFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_jsjournalstore_free(ptr, 1));
const JsMiddlewareFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_jsmiddleware_free(ptr, 1));
const JsModelFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_jsmodel_free(ptr, 1));
const JsObserverFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_jsobserver_free(ptr, 1));
const JsRandomSourceFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_jsrandomsource_free(ptr, 1));
const JsToolsetFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_jstoolset_free(ptr, 1));
const LaneFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_lane_free(ptr, 1));
const LocatorFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_locator_free(ptr, 1));
const MemoryExternalIdentityMapFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_memoryexternalidentitymap_free(ptr, 1));
const RunFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_run_free(ptr, 1));
const RunResultFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_runresult_free(ptr, 1));
const SessionFinalization = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(ptr => wasm.__wbg_session_free(ptr, 1));

function addHeapObject(obj) {
    if (heap_next === heap.length) heap.push(heap.length + 1);
    const idx = heap_next;
    heap_next = heap[idx];

    heap[idx] = obj;
    return idx;
}

function _assertClass(instance, klass) {
    if (!(instance instanceof klass)) {
        throw new Error(`expected instance of ${klass.name}`);
    }
}

const CLOSURE_DTORS = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(state => wasm.__wbindgen_export4(state.a, state.b));

function dropObject(idx) {
    if (idx < 1028) return;
    heap[idx] = heap_next;
    heap_next = idx;
}

function getArrayJsValueFromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    const mem = getDataViewMemory0();
    const result = [];
    for (let i = ptr; i < ptr + 4 * len; i += 4) {
        result.push(takeObject(mem.getUint32(i, true)));
    }
    return result;
}

function getArrayU8FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint8ArrayMemory0().subarray(ptr / 1, ptr / 1 + len);
}

let cachedDataViewMemory0 = null;
function getDataViewMemory0() {
    if (cachedDataViewMemory0 === null || cachedDataViewMemory0.buffer.detached === true || (cachedDataViewMemory0.buffer.detached === undefined && cachedDataViewMemory0.buffer !== wasm.memory.buffer)) {
        cachedDataViewMemory0 = new DataView(wasm.memory.buffer);
    }
    return cachedDataViewMemory0;
}

function getStringFromWasm0(ptr, len) {
    return decodeText(ptr >>> 0, len);
}

let cachedUint8ArrayMemory0 = null;
function getUint8ArrayMemory0() {
    if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
        cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
    }
    return cachedUint8ArrayMemory0;
}

function getObject(idx) { return heap[idx]; }

function handleError(f, args) {
    try {
        return f.apply(this, args);
    } catch (e) {
        wasm.__wbindgen_export3(addHeapObject(e));
    }
}

let heap = new Array(1024).fill(undefined);
heap.push(undefined, null, true, false);

let heap_next = heap.length;

function isLikeNone(x) {
    return x === undefined || x === null;
}

function makeMutClosure(arg0, arg1, f) {
    const state = { a: arg0, b: arg1, cnt: 1 };
    const real = (...args) => {

        // First up with a closure we increment the internal reference
        // count. This ensures that the Rust closure environment won't
        // be deallocated while we're invoking it.
        state.cnt++;
        const a = state.a;
        state.a = 0;
        try {
            return f(a, state.b, ...args);
        } finally {
            state.a = a;
            real._wbg_cb_unref();
        }
    };
    real._wbg_cb_unref = () => {
        if (--state.cnt === 0) {
            wasm.__wbindgen_export4(state.a, state.b);
            state.a = 0;
            CLOSURE_DTORS.unregister(state);
        }
    };
    CLOSURE_DTORS.register(real, state, state);
    return real;
}

function passArrayJsValueToWasm0(array, malloc) {
    const ptr = malloc(array.length * 4, 4) >>> 0;
    const mem = getDataViewMemory0();
    for (let i = 0; i < array.length; i++) {
        mem.setUint32(ptr + 4 * i, addHeapObject(array[i]), true);
    }
    WASM_VECTOR_LEN = array.length;
    return ptr;
}

function passStringToWasm0(arg, malloc, realloc) {
    if (realloc === undefined) {
        const buf = cachedTextEncoder.encode(arg);
        const ptr = malloc(buf.length, 1) >>> 0;
        getUint8ArrayMemory0().subarray(ptr, ptr + buf.length).set(buf);
        WASM_VECTOR_LEN = buf.length;
        return ptr;
    }

    let len = arg.length;
    let ptr = malloc(len, 1) >>> 0;

    const mem = getUint8ArrayMemory0();

    let offset = 0;

    for (; offset < len; offset++) {
        const code = arg.charCodeAt(offset);
        if (code > 0x7F) break;
        mem[ptr + offset] = code;
    }
    if (offset !== len) {
        if (offset !== 0) {
            arg = arg.slice(offset);
        }
        ptr = realloc(ptr, len, len = offset + arg.length * 3, 1) >>> 0;
        const view = getUint8ArrayMemory0().subarray(ptr + offset, ptr + len);
        const ret = cachedTextEncoder.encodeInto(arg, view);

        offset += ret.written;
        ptr = realloc(ptr, len, offset, 1) >>> 0;
    }

    WASM_VECTOR_LEN = offset;
    return ptr;
}

function takeObject(idx) {
    const ret = getObject(idx);
    dropObject(idx);
    return ret;
}

let cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
cachedTextDecoder.decode();
const MAX_SAFARI_DECODE_BYTES = 2146435072;
let numBytesDecoded = 0;
function decodeText(ptr, len) {
    numBytesDecoded += len;
    if (numBytesDecoded >= MAX_SAFARI_DECODE_BYTES) {
        cachedTextDecoder = new TextDecoder('utf-8', { ignoreBOM: true, fatal: true });
        cachedTextDecoder.decode();
        numBytesDecoded = len;
    }
    return cachedTextDecoder.decode(getUint8ArrayMemory0().subarray(ptr, ptr + len));
}

const cachedTextEncoder = new TextEncoder();

if (!('encodeInto' in cachedTextEncoder)) {
    cachedTextEncoder.encodeInto = function (arg, view) {
        const buf = cachedTextEncoder.encode(arg);
        view.set(buf);
        return {
            read: arg.length,
            written: buf.length
        };
    };
}

let WASM_VECTOR_LEN = 0;

let wasmModule, wasmInstance, wasm;
function __wbg_finalize_init(instance, module) {
    wasmInstance = instance;
    wasm = instance.exports;
    wasmModule = module;
    cachedDataViewMemory0 = null;
    cachedUint8ArrayMemory0 = null;
    wasm.__wbindgen_start();
    return wasm;
}

async function __wbg_load(module, imports) {
    if (typeof Response === 'function' && module instanceof Response) {
        if (typeof WebAssembly.instantiateStreaming === 'function') {
            try {
                return await WebAssembly.instantiateStreaming(module, imports);
            } catch (e) {
                const validResponse = module.ok && expectedResponseType(module.type);

                if (validResponse && module.headers.get('Content-Type') !== 'application/wasm') {
                    console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);

                } else { throw e; }
            }
        }

        const bytes = await module.arrayBuffer();
        return await WebAssembly.instantiate(bytes, imports);
    } else {
        const instance = await WebAssembly.instantiate(module, imports);

        if (instance instanceof WebAssembly.Instance) {
            return { instance, module };
        } else {
            return instance;
        }
    }

    function expectedResponseType(type) {
        switch (type) {
            case 'basic': case 'cors': case 'default': return true;
        }
        return false;
    }
}

function initSync(module) {
    if (wasm !== undefined) return wasm;


    if (module !== undefined) {
        if (Object.getPrototypeOf(module) === Object.prototype) {
            ({module} = module)
        } else {
            console.warn('using deprecated parameters for `initSync()`; pass a single object instead')
        }
    }

    const imports = __wbg_get_imports();
    if (!(module instanceof WebAssembly.Module)) {
        module = new WebAssembly.Module(module);
    }
    const instance = new WebAssembly.Instance(module, imports);
    return __wbg_finalize_init(instance, module);
}

async function __wbg_init(module_or_path) {
    if (wasm !== undefined) return wasm;


    if (module_or_path !== undefined) {
        if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
            ({module_or_path} = module_or_path)
        } else {
            console.warn('using deprecated parameters for the initialization function; pass a single object instead')
        }
    }

    if (module_or_path === undefined) {
        module_or_path = new URL('finstack_ai_wasm_bg.wasm', import.meta.url);
    }
    const imports = __wbg_get_imports();

    if (typeof module_or_path === 'string' || (typeof Request === 'function' && module_or_path instanceof Request) || (typeof URL === 'function' && module_or_path instanceof URL)) {
        module_or_path = fetch(module_or_path);
    }

    const { instance, module } = await __wbg_load(await module_or_path, imports);

    return __wbg_finalize_init(instance, module);
}

export { initSync, __wbg_init as default };
