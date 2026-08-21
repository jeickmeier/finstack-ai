/**
 * Experimental same-origin IndexedDB journal and artifact batteries.
 *
 * Persistence is origin-scoped and not crash-durable. `health().detail` stays
 * `js_indexeddb_experimental` and does not claim crash durability.
 * Schema version remains 1.
 */
import { FinstackError } from "../errors.js";
/** Default experimental database name. */
export const INDEXED_DB_NAME = "finstack-ai-experimental";
/** Provisional IndexedDB schema version. */
export const INDEXED_DB_SCHEMA_VERSION = 1;
const ZERO_DIGEST = "0".repeat(64);
const DEFAULT_MAX_SNAPSHOT_BYTES = 65_536;
const DEFAULT_MAX_BATCHES = 256;
const DEFAULT_MAX_RECORDS = 4_096;
const DEFAULT_MAX_SESSIONS = 64;
const DEFAULT_MAX_BLOB_BYTES = 262_144;
const DEFAULT_MAX_BLOBS = 32;
/**
 * Create an experimental IndexedDB journal store.
 *
 * @param options - Database name and resource ceilings.
 * @returns A host journal that never claims durability.
 * @example
 * ```ts
 * const store = createIndexedDbJournalStore();
 * ```
 */
export function createIndexedDbJournalStore(options = {}) {
    const resolved = resolveOptions(options);
    const persistence = createIndexedDbPersistence(resolved.dbName);
    return createJournalStore(persistence, resolved.journal, "js_indexeddb_experimental");
}
/**
 * Create an in-memory host journal that implements the same CAS contract.
 *
 * Used by in-page proofs. This is not durable and is not IndexedDB.
 *
 * @param options - Resource ceilings.
 * @returns A Map-backed host journal.
 */
export function createMapJournalStore(options = {}) {
    const resolved = resolveOptions(options);
    return createJournalStore(createMapPersistence(), resolved.journal, "js_map_experimental");
}
/**
 * Create an experimental IndexedDB artifact store.
 *
 * Agent composition does not yet consume artifacts. put/get/reload/delete are
 * host-local proofs on the existing `HostArtifactStore` port.
 *
 * @param options - Database name and blob ceilings.
 * @returns A host artifact store.
 * @example
 * ```ts
 * const artifacts = createIndexedDbArtifactStore();
 * ```
 */
export function createIndexedDbArtifactStore(options = {}) {
    const resolved = resolveOptions(options);
    return createArtifactStore(resolved.dbName, resolved.maxBlobBytes, resolved.maxBlobs);
}
/**
 * Delete the experimental IndexedDB database for this origin.
 *
 * @param options - Optional database name.
 * @returns A promise that settles after deletion is requested.
 * @example
 * ```ts
 * await deleteIndexedDbStores();
 * ```
 */
export async function deleteIndexedDbStores(options = {}) {
    const dbName = options.dbName ?? INDEXED_DB_NAME;
    await deleteDatabase(dbName);
}
function resolveOptions(options) {
    return {
        dbName: options.dbName ?? INDEXED_DB_NAME,
        journal: {
            maxSnapshotBytes: options.maxSnapshotBytes ?? DEFAULT_MAX_SNAPSHOT_BYTES,
            maxBatchesPerSession: options.maxBatchesPerSession ?? DEFAULT_MAX_BATCHES,
            maxRecordsPerSession: options.maxRecordsPerSession ?? DEFAULT_MAX_RECORDS,
            maxSessions: DEFAULT_MAX_SESSIONS,
        },
        maxBlobBytes: options.maxBlobBytes ?? DEFAULT_MAX_BLOB_BYTES,
        maxBlobs: options.maxBlobs ?? DEFAULT_MAX_BLOBS,
    };
}
function createJournalStore(persistence, limits, detail) {
    return {
        async health() {
            await persistence.getSchemaVersion();
            return { ready: true, detail };
        },
        async append(request, _options) {
            const raw = asJsonString(request);
            const parsed = parseAppendRequest(raw);
            return appendJournal(persistence, limits, raw, parsed);
        },
        async load(request, _options) {
            const sessionId = parseSessionId(request);
            return loadJournal(persistence, sessionId);
        },
        async writeSnapshot(request, _options) {
            return writeJournalSnapshot(persistence, limits, request);
        },
    };
}
async function appendJournal(persistence, limits, raw, request) {
    await persistence.getSchemaVersion();
    const existing = await persistence.getBatchById(request.batch_id);
    if (existing !== undefined) {
        if (existing.requestDigest === raw) {
            return parseCommitted(existing.committedJson);
        }
        throwStoreError({
            code: "store_corruption",
            reasonCode: "append_batch_id_reuse",
            message: "store corruption: append_batch_id_reuse",
        });
    }
    if (request.records.length === 0) {
        throwStoreError({
            code: "invalid_store_request",
            reasonCode: "empty_append_batch",
            message: "invalid store request: empty_append_batch",
        });
    }
    const batches = await persistence.listBatches(request.session_id);
    const reused = request.records
        .map((record) => findBatchForRecord(batches, record.record_id))
        .filter((row) => row !== undefined);
    if (reused.length > 0) {
        if (reused.length !== request.records.length) {
            throwStoreError({
                code: "store_corruption",
                reasonCode: "mixed_record_id_reuse",
                message: "store corruption: mixed_record_id_reuse",
            });
        }
        const original = reused[0];
        if (original === undefined ||
            reused.some((row) => row.batchId !== original.batchId)) {
            throwStoreError({
                code: "store_corruption",
                reasonCode: "mixed_record_batch_reuse",
                message: "store corruption: mixed_record_batch_reuse",
            });
        }
        if (original.requestDigest === raw) {
            return parseCommitted(original.committedJson);
        }
        throwStoreError({
            code: "store_corruption",
            reasonCode: "record_id_reuse",
            message: "store corruption: record_id_reuse",
        });
    }
    const session = await persistence.getSession(request.session_id);
    const currentHead = session?.headSequence ?? 0;
    const actualNext = currentHead + 1;
    if (request.expected_sequence !== actualNext) {
        throwStoreError({
            code: "store_conflict",
            expectedSequence: request.expected_sequence,
            actualNextSequence: actualNext,
            message: `store conflict: expected sequence ${request.expected_sequence}, actual next sequence ${actualNext}`,
        });
    }
    const sessionIsNew = session === undefined;
    if (sessionIsNew) {
        const sessions = await persistence.listSessions();
        if (sessions.length >= limits.maxSessions) {
            throwStoreError({
                code: "store_limit_exceeded",
                resource: "sessions",
                limit: limits.maxSessions,
                message: `store limit exceeded for sessions: ${limits.maxSessions}`,
            });
        }
    }
    if (session !== undefined && batches.length >= limits.maxBatchesPerSession) {
        throwStoreError({
            code: "store_limit_exceeded",
            resource: "batches_per_session",
            limit: limits.maxBatchesPerSession,
            message: `store limit exceeded for batches_per_session: ${limits.maxBatchesPerSession}`,
        });
    }
    const nextRecordCount = (session?.recordCount ?? 0) + request.records.length;
    if (nextRecordCount > limits.maxRecordsPerSession) {
        throwStoreError({
            code: "store_limit_exceeded",
            resource: "records_per_session",
            limit: limits.maxRecordsPerSession,
            message: `store limit exceeded for records_per_session: ${limits.maxRecordsPerSession}`,
        });
    }
    const committed = commitBatch(request);
    const committedJson = JSON.stringify(committed);
    if (committedJson.length > limits.maxSnapshotBytes * 4) {
        throwStoreError({
            code: "store_limit_exceeded",
            resource: "batches_per_session",
            limit: limits.maxSnapshotBytes * 4,
            message: "store limit exceeded for batch bytes",
        });
    }
    await persistence.commitAppend({
        sessionId: request.session_id,
        firstSequence: committed.first_sequence,
        batchId: request.batch_id,
        requestDigest: raw,
        committedJson,
        recordIds: request.records.map((record) => record.record_id),
    }, {
        sessionId: request.session_id,
        headSequence: committed.last_sequence,
        recordCount: nextRecordCount,
    });
    return committed;
}
async function loadJournal(persistence, sessionId) {
    await persistence.getSchemaVersion();
    const session = await persistence.getSession(sessionId);
    if (session === undefined) {
        return {
            session_id: sessionId,
            head_sequence: 0,
            committed_batches: [],
        };
    }
    const batches = (await persistence.listBatches(sessionId)).sort((left, right) => left.firstSequence - right.firstSequence);
    const committed = batches.map((row) => parseCommitted(row.committedJson));
    const snapshot = await persistence.getSnapshot(sessionId);
    return {
        session_id: sessionId,
        head_sequence: session.headSequence,
        committed_batches: committed,
        ...(snapshot === undefined
            ? {}
            : {
                snapshot: {
                    sequence: snapshot.sequence,
                    digest: snapshot.digest,
                    bytes: snapshot.bytes,
                },
            }),
    };
}
async function writeJournalSnapshot(persistence, limits, request) {
    await persistence.getSchemaVersion();
    const parsed = parseJsonObject(request);
    const sessionId = requiredString(parsed, "session_id");
    const snapshot = parsed.snapshot;
    if (snapshot === null || typeof snapshot !== "object") {
        throwStoreError({
            code: "invalid_store_request",
            reasonCode: "snapshot_session_not_found",
            message: "invalid store request: snapshot_session_not_found",
        });
    }
    const snapshotRecord = snapshot;
    const sequence = requiredNumber(snapshotRecord, "sequence");
    const digest = requiredString(snapshotRecord, "digest");
    const bytes = requiredByteArray(snapshotRecord.bytes);
    if (bytes.length > limits.maxSnapshotBytes) {
        throwStoreError({
            code: "store_limit_exceeded",
            resource: "snapshot_bytes",
            limit: limits.maxSnapshotBytes,
            message: `store limit exceeded for snapshot_bytes: ${limits.maxSnapshotBytes}`,
        });
    }
    const session = await persistence.getSession(sessionId);
    if (session === undefined) {
        throwStoreError({
            code: "invalid_store_request",
            reasonCode: "snapshot_session_not_found",
            message: "invalid store request: snapshot_session_not_found",
        });
    }
    if (sequence > session.headSequence) {
        throwStoreError({
            code: "invalid_store_request",
            reasonCode: "snapshot_ahead_of_journal",
            message: "invalid store request: snapshot_ahead_of_journal",
        });
    }
    const current = await persistence.getSnapshot(sessionId);
    if (current !== undefined && current.sequence > sequence) {
        throwStoreError({
            code: "invalid_store_request",
            reasonCode: "snapshot_sequence_regression",
            message: "invalid store request: snapshot_sequence_regression",
        });
    }
    await persistence.putSnapshot({
        sessionId,
        sequence,
        digest,
        bytes,
    });
    return {
        session_id: sessionId,
        sequence,
        digest,
        bytes: bytes.length,
    };
}
function commitBatch(request) {
    const records = request.records.map((draft, offset) => {
        const record = {
            format_version: draft.format_version,
            kind_version: draft.kind_version,
            record_id: draft.record_id,
            session_id: draft.session_id,
            lane_id: draft.lane_id,
            sequence: request.expected_sequence + offset,
            timestamp: draft.timestamp,
            payload_digest: ZERO_DIGEST,
            checksum: ZERO_DIGEST,
            derived_event_ids: draft.derived_event_ids,
            body: draft.body,
        };
        if (draft.run_id !== undefined) {
            record.run_id = draft.run_id;
        }
        if (offset > 0) {
            record.previous_checksum = ZERO_DIGEST;
        }
        return record;
    });
    const last = records[records.length - 1];
    return {
        batch_id: request.batch_id,
        first_sequence: request.expected_sequence,
        last_sequence: last?.sequence ?? request.expected_sequence,
        records,
    };
}
function findBatchForRecord(batches, recordId) {
    return batches.find((batch) => batch.recordIds.includes(recordId));
}
function parseCommitted(encoded) {
    try {
        const parsed = JSON.parse(encoded);
        if (parsed === null || typeof parsed !== "object") {
            throw new Error("not an object");
        }
        return parsed;
    }
    catch {
        throwStoreError({
            code: "store_integrity_failure",
            reasonCode: "journal_record_corrupt",
            message: "store integrity failure: journal_record_corrupt",
        });
    }
}
function parseAppendRequest(raw) {
    const parsed = parseJsonObject(raw);
    const records = parsed.records;
    if (!Array.isArray(records)) {
        throwStoreError({
            code: "invalid_store_request",
            reasonCode: "empty_append_batch",
            message: "invalid store request: empty_append_batch",
        });
    }
    return {
        batch_id: requiredString(parsed, "batch_id"),
        session_id: requiredString(parsed, "session_id"),
        expected_sequence: requiredNumber(parsed, "expected_sequence"),
        records: records,
    };
}
function parseSessionId(request) {
    return requiredString(parseJsonObject(request), "session_id");
}
function asJsonString(value) {
    return typeof value === "string" ? value : JSON.stringify(value);
}
function parseJsonObject(value) {
    const parsed = typeof value === "string" ? JSON.parse(value) : value;
    if (parsed === null || typeof parsed !== "object") {
        throwStoreError({
            code: "store_integrity_failure",
            reasonCode: "journal_record_corrupt",
            message: "store integrity failure: journal_record_corrupt",
        });
    }
    return parsed;
}
function requiredString(record, key) {
    const value = record[key];
    if (typeof value !== "string" || value.length === 0) {
        throwStoreError({
            code: "store_integrity_failure",
            reasonCode: "journal_record_corrupt",
            message: `store integrity failure: journal_record_corrupt`,
        });
    }
    return value;
}
function requiredNumber(record, key) {
    const value = record[key];
    if (typeof value !== "number" || !Number.isFinite(value)) {
        throwStoreError({
            code: "store_integrity_failure",
            reasonCode: "journal_record_corrupt",
            message: "store integrity failure: journal_record_corrupt",
        });
    }
    return value;
}
function requiredByteArray(value) {
    if (!Array.isArray(value) || value.some((item) => typeof item !== "number")) {
        throwStoreError({
            code: "store_integrity_failure",
            reasonCode: "journal_record_corrupt",
            message: "store integrity failure: journal_record_corrupt",
        });
    }
    return value;
}
function throwStoreError(error) {
    const thrown = new FinstackError(error.message, {
        code: error.code,
        retryable: false,
    });
    if (error.reasonCode !== undefined) {
        Object.assign(thrown, { reasonCode: error.reasonCode });
    }
    if (error.expectedSequence !== undefined) {
        Object.assign(thrown, { expectedSequence: error.expectedSequence });
    }
    if (error.actualNextSequence !== undefined) {
        Object.assign(thrown, { actualNextSequence: error.actualNextSequence });
    }
    if (error.resource !== undefined) {
        Object.assign(thrown, { resource: error.resource });
    }
    if (error.limit !== undefined) {
        Object.assign(thrown, { limit: error.limit });
    }
    throw thrown;
}
function createMapPersistence() {
    const sessions = new Map();
    const batches = new Map();
    const snapshots = new Map();
    let lastSessionId;
    return {
        async getSchemaVersion() {
            return INDEXED_DB_SCHEMA_VERSION;
        },
        async listSessions() {
            return [...sessions.values()];
        },
        async getSession(sessionId) {
            return sessions.get(sessionId);
        },
        async putSession(row) {
            sessions.set(row.sessionId, row);
        },
        async getBatchById(batchId) {
            return [...batches.values()].find((row) => row.batchId === batchId);
        },
        async listBatches(sessionId) {
            return [...batches.values()].filter((row) => row.sessionId === sessionId);
        },
        async putBatch(row) {
            batches.set(`${row.sessionId}:${row.firstSequence}`, row);
        },
        async getSnapshot(sessionId) {
            return snapshots.get(sessionId);
        },
        async putSnapshot(row) {
            snapshots.set(row.sessionId, row);
        },
        async getLastSessionId() {
            return lastSessionId;
        },
        async setLastSessionId(sessionId) {
            lastSessionId = sessionId;
        },
        async commitAppend(batch, session) {
            batches.set(`${batch.sessionId}:${batch.firstSequence}`, batch);
            sessions.set(session.sessionId, session);
            lastSessionId = session.sessionId;
        },
    };
}
function createIndexedDbPersistence(dbName) {
    const opened = openExperimentalDb(dbName);
    return {
        async getSchemaVersion() {
            const db = await opened;
            return readSchemaVersion(db);
        },
        async listSessions() {
            const db = await opened;
            return idbGetAll(db, "sessions");
        },
        async getSession(sessionId) {
            const db = await opened;
            return idbGet(db, "sessions", sessionId);
        },
        async putSession(row) {
            const db = await opened;
            await idbPut(db, "sessions", row);
        },
        async getBatchById(batchId) {
            const db = await opened;
            return idbGetFromIndex(db, "batches", "batchId", batchId);
        },
        async listBatches(sessionId) {
            const db = await opened;
            return idbGetAllInRange(db, "batches", IDBKeyRange.bound([sessionId, 0], [sessionId, Number.MAX_SAFE_INTEGER]));
        },
        async putBatch(row) {
            const db = await opened;
            await idbPut(db, "batches", row);
        },
        async getSnapshot(sessionId) {
            const db = await opened;
            return idbGet(db, "snapshots", sessionId);
        },
        async putSnapshot(row) {
            const db = await opened;
            await idbPut(db, "snapshots", row);
        },
        async getLastSessionId() {
            const db = await opened;
            const row = await idbGet(db, "meta", "lastSessionId");
            return row?.value;
        },
        async setLastSessionId(sessionId) {
            const db = await opened;
            await idbPut(db, "meta", { key: "lastSessionId", value: sessionId });
        },
        async commitAppend(batch, session) {
            const db = await opened;
            await idbWrite(db, ["batches", "sessions", "meta"], (txn) => {
                txn.objectStore("batches").put(batch);
                txn.objectStore("sessions").put(session);
                txn.objectStore("meta").put({ key: "lastSessionId", value: session.sessionId });
            });
        },
    };
}
function createArtifactStore(dbName, maxBlobBytes, maxBlobs) {
    const opened = openExperimentalDb(dbName);
    return {
        async stagePut(_scope, content, _metadata, id) {
            const db = await opened;
            await readSchemaVersion(db);
            if (content.byteLength > maxBlobBytes) {
                throwStoreError({
                    code: "store_limit_exceeded",
                    resource: "blob_bytes",
                    limit: maxBlobBytes,
                    message: `store limit exceeded for blob_bytes: ${maxBlobBytes}`,
                });
            }
            const existing = await idbGetAll(db, "artifacts");
            const key = String(id ?? "");
            if (key.length === 0) {
                throwStoreError({
                    code: "invalid_store_request",
                    reasonCode: "artifact_id_missing",
                    message: "invalid store request: artifact_id_missing",
                });
            }
            if (!existing.some((row) => row.id === key) && existing.length >= maxBlobs) {
                throwStoreError({
                    code: "store_limit_exceeded",
                    resource: "blobs",
                    limit: maxBlobs,
                    message: `store limit exceeded for blobs: ${maxBlobs}`,
                });
            }
            await idbPut(db, "artifacts", {
                id: key,
                bytes: Array.from(content),
                byteLength: content.byteLength,
            });
        },
        async get(key) {
            const db = await opened;
            await readSchemaVersion(db);
            const row = await idbGet(db, "artifacts", String(key));
            if (row === undefined) {
                throwStoreError({
                    code: "invalid_store_request",
                    reasonCode: "artifact_uncorrelated",
                    message: "invalid store request: artifact_uncorrelated",
                });
            }
            return Uint8Array.from(row.bytes);
        },
    };
}
function openExperimentalDb(dbName) {
    return new Promise((resolve, reject) => {
        let request;
        try {
            request = indexedDB.open(dbName, INDEXED_DB_SCHEMA_VERSION);
        }
        catch (error) {
            reject(asSchemaError(error));
            return;
        }
        request.onerror = () => {
            reject(asSchemaError(request.error));
        };
        request.onupgradeneeded = (event) => {
            const db = request.result;
            if (event.oldVersion > INDEXED_DB_SCHEMA_VERSION) {
                db.close();
                reject(schemaUnsupported());
                return;
            }
            if (!db.objectStoreNames.contains("meta")) {
                db.createObjectStore("meta", { keyPath: "key" });
            }
            if (!db.objectStoreNames.contains("sessions")) {
                db.createObjectStore("sessions", { keyPath: "sessionId" });
            }
            if (!db.objectStoreNames.contains("batches")) {
                const batches = db.createObjectStore("batches", {
                    keyPath: ["sessionId", "firstSequence"],
                });
                batches.createIndex("batchId", "batchId", { unique: true });
            }
            if (!db.objectStoreNames.contains("snapshots")) {
                db.createObjectStore("snapshots", { keyPath: "sessionId" });
            }
            if (!db.objectStoreNames.contains("artifacts")) {
                db.createObjectStore("artifacts", { keyPath: "id" });
            }
        };
        request.onsuccess = () => {
            const db = request.result;
            if (db.version > INDEXED_DB_SCHEMA_VERSION) {
                db.close();
                reject(schemaUnsupported());
                return;
            }
            void readSchemaVersion(db)
                .then(() => resolve(db))
                .catch(reject);
        };
    });
}
async function readSchemaVersion(db) {
    const row = await idbGet(db, "meta", "schemaVersion");
    if (row === undefined) {
        await idbPut(db, "meta", {
            key: "schemaVersion",
            value: INDEXED_DB_SCHEMA_VERSION,
        });
        return INDEXED_DB_SCHEMA_VERSION;
    }
    if (row.value !== INDEXED_DB_SCHEMA_VERSION) {
        throw schemaUnsupported();
    }
    return row.value;
}
function schemaUnsupported() {
    const error = new FinstackError("store integrity failure: journal_schema_unsupported", {
        code: "store_integrity_failure",
        retryable: false,
    });
    Object.assign(error, { reasonCode: "journal_schema_unsupported" });
    return error;
}
function asSchemaError(error) {
    if (error instanceof DOMException && error.name === "VersionError") {
        return schemaUnsupported();
    }
    if (error instanceof FinstackError) {
        return error;
    }
    return schemaUnsupported();
}
function deleteDatabase(dbName) {
    return new Promise((resolve, reject) => {
        const request = indexedDB.deleteDatabase(dbName);
        request.onsuccess = () => resolve();
        request.onerror = () => reject(request.error ?? new Error("indexeddb delete failed"));
        request.onblocked = () => resolve();
    });
}
function idbGet(db, store, key) {
    return new Promise((resolve, reject) => {
        const request = db.transaction(store, "readonly").objectStore(store).get(key);
        request.onsuccess = () => resolve(request.result);
        request.onerror = () => reject(request.error ?? new Error("indexeddb get failed"));
    });
}
function idbGetFromIndex(db, store, index, key) {
    return new Promise((resolve, reject) => {
        const request = db
            .transaction(store, "readonly")
            .objectStore(store)
            .index(index)
            .get(key);
        request.onsuccess = () => resolve(request.result);
        request.onerror = () => reject(request.error ?? new Error("indexeddb index get failed"));
    });
}
function idbGetAll(db, store) {
    return new Promise((resolve, reject) => {
        const request = db.transaction(store, "readonly").objectStore(store).getAll();
        request.onsuccess = () => resolve(request.result ?? []);
        request.onerror = () => reject(request.error ?? new Error("indexeddb getAll failed"));
    });
}
function idbPut(db, store, value) {
    return idbWrite(db, [store], (txn) => {
        txn.objectStore(store).put(value);
    });
}
function idbWrite(db, stores, mutate) {
    return new Promise((resolve, reject) => {
        const txn = db.transaction(stores, "readwrite");
        txn.oncomplete = () => resolve();
        txn.onerror = () => reject(txn.error ?? new Error("indexeddb write failed"));
        txn.onabort = () => reject(txn.error ?? new Error("indexeddb write aborted"));
        mutate(txn);
    });
}
function idbGetAllInRange(db, store, range) {
    return new Promise((resolve, reject) => {
        const request = db.transaction(store, "readonly").objectStore(store).getAll(range);
        request.onsuccess = () => resolve(request.result ?? []);
        request.onerror = () => reject(request.error ?? new Error("indexeddb range getAll failed"));
    });
}
//# sourceMappingURL=indexeddb.js.map