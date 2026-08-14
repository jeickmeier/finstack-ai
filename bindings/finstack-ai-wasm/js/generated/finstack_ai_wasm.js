/* @ts-self-types="./finstack_ai_wasm.d.ts" */

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
 * Pre-beta in-memory JS journal-store wrapper. Not crash-durable.
 */
export class JsJournalStore {
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
     * Construct a scripted journal-store wrapper.
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
        __wbg_new_116be93542d39019: function() {
            const ret = new Array();
            return addHeapObject(ret);
        },
        __wbg_new_15ae2532051588db: function(arg0, arg1) {
            const ret = new TypeError(getStringFromWasm0(arg0, arg1));
            return addHeapObject(ret);
        },
        __wbg_new_ebe3e0f6837f0879: function() {
            const ret = new Object();
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
                        return __wasm_bindgen_func_elem_874(a, state0.b, arg0, arg1);
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
        __wbg_resolve_020f95d838c6ef25: function(arg0) {
            const ret = Promise.resolve(getObject(arg0));
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
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [Externref], shim_idx: 222, ret: Result(Unit), inner_ret: Some(Result(Unit)) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, __wasm_bindgen_func_elem_861);
            return addHeapObject(ret);
        },
        __wbindgen_cast_0000000000000002: function(arg0, arg1) {
            // Cast intrinsic for `Closure(Closure { owned: true, function: Function { arguments: [], shim_idx: 5, ret: Unit, inner_ret: Some(Unit) }, mutable: true }) -> Externref`.
            const ret = makeMutClosure(arg0, arg1, __wasm_bindgen_func_elem_193);
            return addHeapObject(ret);
        },
        __wbindgen_cast_0000000000000003: function(arg0, arg1) {
            // Cast intrinsic for `Ref(String) -> Externref`.
            const ret = getStringFromWasm0(arg0, arg1);
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

function __wasm_bindgen_func_elem_193(arg0, arg1) {
    wasm.__wasm_bindgen_func_elem_193(arg0, arg1);
}

function __wasm_bindgen_func_elem_861(arg0, arg1, arg2) {
    try {
        const retptr = wasm.__wbindgen_add_to_stack_pointer(-16);
        wasm.__wasm_bindgen_func_elem_861(retptr, arg0, arg1, addHeapObject(arg2));
        var r0 = getDataViewMemory0().getInt32(retptr + 4 * 0, true);
        var r1 = getDataViewMemory0().getInt32(retptr + 4 * 1, true);
        if (r1) {
            throw takeObject(r0);
        }
    } finally {
        wasm.__wbindgen_add_to_stack_pointer(16);
    }
}

function __wasm_bindgen_func_elem_874(arg0, arg1, arg2, arg3) {
    wasm.__wasm_bindgen_func_elem_874(arg0, arg1, addHeapObject(arg2), addHeapObject(arg3));
}

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

function addHeapObject(obj) {
    if (heap_next === heap.length) heap.push(heap.length + 1);
    const idx = heap_next;
    heap_next = heap[idx];

    heap[idx] = obj;
    return idx;
}

const CLOSURE_DTORS = (typeof FinalizationRegistry === 'undefined')
    ? { register: () => {}, unregister: () => {} }
    : new FinalizationRegistry(state => wasm.__wbindgen_export4(state.a, state.b));

function dropObject(idx) {
    if (idx < 1028) return;
    heap[idx] = heap_next;
    heap_next = idx;
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
