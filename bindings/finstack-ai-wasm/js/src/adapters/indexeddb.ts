/**
 * Experimental same-origin IndexedDB journal and artifact batteries.
 *
 * Persistence is origin-scoped and not crash-durable. `health().detail` stays
 * `js_indexeddb_experimental` and does not claim crash durability.
 * Schema version 2 stores exact scoped artifact references.
 */

import { FinstackError } from "../errors.js";
import type { HostArtifactStore, HostCallOptions, HostJournalStore } from "../host.js";

/** Default experimental database name. */
export const INDEXED_DB_NAME = "finstack-ai-experimental";

/** Provisional IndexedDB schema version. */
export const INDEXED_DB_SCHEMA_VERSION = 2;

const ZERO_DIGEST = "0".repeat(64);
const DEFAULT_MAX_SNAPSHOT_BYTES = 65_536;
const DEFAULT_MAX_BATCHES = 256;
const DEFAULT_MAX_RECORDS = 4_096;
const DEFAULT_MAX_SESSIONS = 64;
const DEFAULT_MAX_BLOB_BYTES = 262_144;
const DEFAULT_MAX_BLOBS = 32;
const DEFAULT_MAX_TOTAL_BLOB_BYTES = 8_388_608;
const DEFAULT_MAX_ARTIFACT_OWNERS = 128;
const DEFAULT_ARTIFACT_ORPHAN_GRACE_MS = 300_000;
const DEFAULT_MAX_ARTIFACT_GC_BATCH = 128;

/**
 * Options for the experimental IndexedDB batteries.
 */
export interface IndexedDbStoreOptions {
  /** Database name. Defaults to {@link INDEXED_DB_NAME}. */
  dbName?: string;
  /** Rejected. Queue capacity belongs to the worker protocol, not the store. */
  queueCapacity?: never;
  /** Snapshot byte ceiling. Defaults to 65536. */
  maxSnapshotBytes?: number;
  /** Committed-batch ceiling per session. Defaults to 256. */
  maxBatchesPerSession?: number;
  /** Committed-record ceiling per session. Defaults to 4096. */
  maxRecordsPerSession?: number;
  /** Artifact byte ceiling. Defaults to 262144. */
  maxBlobBytes?: number;
  /** Artifact count ceiling. Defaults to 32. */
  maxBlobs?: number;
  /** Aggregate artifact byte ceiling. Defaults to 8388608. */
  maxTotalBlobBytes?: number;
  /** Maximum owners retaining one artifact. Defaults to 128. */
  maxArtifactOwners?: number;
  /** Unowned artifact grace period in milliseconds. Defaults to 300000. */
  artifactOrphanGraceMs?: number;
  /** Maximum artifacts examined by one collection call. Defaults to 128. */
  maxArtifactGcBatch?: number;
}

interface JournalLimits {
  maxSnapshotBytes: number;
  maxBatchesPerSession: number;
  maxRecordsPerSession: number;
  maxSessions: number;
}

interface ArtifactRow {
  storageKey: string;
  scope: string;
  artifact: string;
  bytes: number[];
  byteLength: number;
  owners: string[];
  unreferencedSince?: number;
}

interface RecordDraftJson {
  format_version: number;
  kind_version: number;
  record_id: string;
  session_id: string;
  lane_id: string;
  run_id?: string;
  timestamp: string;
  derived_event_ids: string[];
  body: unknown;
}

interface AppendRequestJson {
  batch_id: string;
  session_id: string;
  expected_sequence: number;
  records: RecordDraftJson[];
}

interface CommittedRecordJson {
  format_version: number;
  kind_version: number;
  record_id: string;
  session_id: string;
  lane_id: string;
  run_id?: string;
  sequence: number;
  timestamp: string;
  payload_digest: string;
  previous_checksum?: string;
  checksum: string;
  derived_event_ids: string[];
  body: unknown;
}

interface CommittedBatchJson {
  batch_id: string;
  first_sequence: number;
  last_sequence: number;
  records: CommittedRecordJson[];
}

interface SessionRow {
  sessionId: string;
  headSequence: number;
  recordCount: number;
}

interface BatchRow {
  sessionId: string;
  firstSequence: number;
  batchId: string;
  requestDigest: string;
  committedJson: string;
  recordIds: string[];
}

interface SnapshotRow {
  sessionId: string;
  sequence: number;
  digest: string;
  bytes: number[];
}

interface JournalPersistence {
  getSchemaVersion(): Promise<number>;
  listSessions(): Promise<SessionRow[]>;
  getSession(sessionId: string): Promise<SessionRow | undefined>;
  putSession(row: SessionRow): Promise<void>;
  getBatchById(batchId: string): Promise<BatchRow | undefined>;
  listBatches(sessionId: string): Promise<BatchRow[]>;
  putBatch(row: BatchRow): Promise<void>;
  getSnapshot(sessionId: string): Promise<SnapshotRow | undefined>;
  putSnapshot(row: SnapshotRow): Promise<void>;
  getLastSessionId(): Promise<string | undefined>;
  setLastSessionId(sessionId: string): Promise<void>;
  commitAppend(batch: BatchRow, session: SessionRow): Promise<void>;
}

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
export function createIndexedDbJournalStore(
  options: IndexedDbStoreOptions = {},
): HostJournalStore {
  const resolved = resolveOptions(options);
  const persistence = createIndexedDbPersistence(resolved.dbName);
  return createJournalStore(
    persistence,
    resolved.journal,
    "js_indexeddb_experimental",
  );
}

/**
 * Create an in-memory host journal that implements the same CAS contract.
 *
 * Used by in-page proofs. This is not durable and is not IndexedDB.
 *
 * @param options - Resource ceilings.
 * @returns A Map-backed host journal.
 */
export function createMapJournalStore(
  options: IndexedDbStoreOptions = {},
): HostJournalStore {
  const resolved = resolveOptions(options);
  return createJournalStore(
    createMapPersistence(),
    resolved.journal,
    "js_map_experimental",
  );
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
export function createIndexedDbArtifactStore(
  options: IndexedDbStoreOptions = {},
): HostArtifactStore {
  const resolved = resolveOptions(options);
  return createArtifactStore(
    resolved.dbName,
    resolved.maxBlobBytes,
    resolved.maxBlobs,
    resolved.maxTotalBlobBytes,
    resolved.maxArtifactOwners,
    resolved.artifactOrphanGraceMs,
    resolved.maxArtifactGcBatch,
  );
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
export async function deleteIndexedDbStores(
  options: Pick<IndexedDbStoreOptions, "dbName"> = {},
): Promise<void> {
  const dbName = options.dbName ?? INDEXED_DB_NAME;
  await deleteDatabase(dbName);
}

function resolveOptions(options: IndexedDbStoreOptions): {
  dbName: string;
  journal: JournalLimits;
  maxBlobBytes: number;
  maxBlobs: number;
  maxTotalBlobBytes: number;
  maxArtifactOwners: number;
  artifactOrphanGraceMs: number;
  maxArtifactGcBatch: number;
} {
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
    maxTotalBlobBytes: options.maxTotalBlobBytes ?? DEFAULT_MAX_TOTAL_BLOB_BYTES,
    maxArtifactOwners: options.maxArtifactOwners ?? DEFAULT_MAX_ARTIFACT_OWNERS,
    artifactOrphanGraceMs:
      options.artifactOrphanGraceMs ?? DEFAULT_ARTIFACT_ORPHAN_GRACE_MS,
    maxArtifactGcBatch: options.maxArtifactGcBatch ?? DEFAULT_MAX_ARTIFACT_GC_BATCH,
  };
}

function createJournalStore(
  persistence: JournalPersistence,
  limits: JournalLimits,
  detail: string,
): HostJournalStore {
  return {
    async health() {
      await persistence.getSchemaVersion();
      return { ready: true, detail };
    },
    async append(request: unknown, _options?: HostCallOptions) {
      const raw = asJsonString(request);
      const parsed = parseAppendRequest(raw);
      return appendJournal(persistence, limits, raw, parsed);
    },
    async load(request: unknown, _options?: HostCallOptions) {
      const sessionId = parseSessionId(request);
      return loadJournal(persistence, sessionId);
    },
    async writeSnapshot(request: unknown, _options?: HostCallOptions) {
      return writeJournalSnapshot(persistence, limits, request);
    },
  };
}

async function appendJournal(
  persistence: JournalPersistence,
  limits: JournalLimits,
  raw: string,
  request: AppendRequestJson,
): Promise<CommittedBatchJson> {
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
    .filter((row): row is BatchRow => row !== undefined);
  if (reused.length > 0) {
    if (reused.length !== request.records.length) {
      throwStoreError({
        code: "store_corruption",
        reasonCode: "mixed_record_id_reuse",
        message: "store corruption: mixed_record_id_reuse",
      });
    }
    const original = reused[0];
    if (
      original === undefined ||
      reused.some((row) => row.batchId !== original.batchId)
    ) {
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
  await persistence.commitAppend(
    {
      sessionId: request.session_id,
      firstSequence: committed.first_sequence,
      batchId: request.batch_id,
      requestDigest: raw,
      committedJson,
      recordIds: request.records.map((record) => record.record_id),
    },
    {
      sessionId: request.session_id,
      headSequence: committed.last_sequence,
      recordCount: nextRecordCount,
    },
  );
  return committed;
}

async function loadJournal(
  persistence: JournalPersistence,
  sessionId: string,
): Promise<unknown> {
  await persistence.getSchemaVersion();
  const session = await persistence.getSession(sessionId);
  if (session === undefined) {
    return {
      session_id: sessionId,
      head_sequence: 0,
      committed_batches: [],
    };
  }
  const batches = (await persistence.listBatches(sessionId)).sort(
    (left, right) => left.firstSequence - right.firstSequence,
  );
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

async function writeJournalSnapshot(
  persistence: JournalPersistence,
  limits: JournalLimits,
  request: unknown,
): Promise<unknown> {
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
  const snapshotRecord = snapshot as Record<string, unknown>;
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

function commitBatch(request: AppendRequestJson): CommittedBatchJson {
  const records = request.records.map((draft, offset) => {
    const record: CommittedRecordJson = {
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

function findBatchForRecord(batches: BatchRow[], recordId: string): BatchRow | undefined {
  return batches.find((batch) => batch.recordIds.includes(recordId));
}

function parseCommitted(encoded: string): CommittedBatchJson {
  try {
    const parsed: unknown = JSON.parse(encoded);
    if (parsed === null || typeof parsed !== "object") {
      throw new Error("not an object");
    }
    return parsed as CommittedBatchJson;
  } catch {
    throwStoreError({
      code: "store_integrity_failure",
      reasonCode: "journal_record_corrupt",
      message: "store integrity failure: journal_record_corrupt",
    });
  }
}

function parseAppendRequest(raw: string): AppendRequestJson {
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
    records: records as RecordDraftJson[],
  };
}

function parseSessionId(request: unknown): string {
  return requiredString(parseJsonObject(request), "session_id");
}

function asJsonString(value: unknown): string {
  return typeof value === "string" ? value : JSON.stringify(value);
}

function parseJsonObject(value: unknown): Record<string, unknown> {
  const parsed: unknown = typeof value === "string" ? JSON.parse(value) : value;
  if (parsed === null || typeof parsed !== "object") {
    throwStoreError({
      code: "store_integrity_failure",
      reasonCode: "journal_record_corrupt",
      message: "store integrity failure: journal_record_corrupt",
    });
  }
  return parsed as Record<string, unknown>;
}

function requiredString(record: Record<string, unknown>, key: string): string {
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

function requiredNumber(record: Record<string, unknown>, key: string): number {
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

function requiredByteArray(value: unknown): number[] {
  if (!Array.isArray(value) || value.some((item) => typeof item !== "number")) {
    throwStoreError({
      code: "store_integrity_failure",
      reasonCode: "journal_record_corrupt",
      message: "store integrity failure: journal_record_corrupt",
    });
  }
  return value as number[];
}

function throwStoreError(error: {
  code: string;
  message: string;
  reasonCode?: string;
  expectedSequence?: number;
  actualNextSequence?: number;
  resource?: string;
  limit?: number;
}): never {
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

function createMapPersistence(): JournalPersistence {
  const sessions = new Map<string, SessionRow>();
  const batches = new Map<string, BatchRow>();
  const snapshots = new Map<string, SnapshotRow>();
  let lastSessionId: string | undefined;
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

function createIndexedDbPersistence(dbName: string): JournalPersistence {
  const opened = openExperimentalDb(dbName);
  return {
    async getSchemaVersion() {
      const db = await opened;
      return readSchemaVersion(db);
    },
    async listSessions() {
      const db = await opened;
      return idbGetAll<SessionRow>(db, "sessions");
    },
    async getSession(sessionId) {
      const db = await opened;
      return idbGet<SessionRow>(db, "sessions", sessionId);
    },
    async putSession(row) {
      const db = await opened;
      await idbPut(db, "sessions", row);
    },
    async getBatchById(batchId) {
      const db = await opened;
      return idbGetFromIndex<BatchRow>(db, "batches", "batchId", batchId);
    },
    async listBatches(sessionId) {
      const db = await opened;
      return idbGetAllInRange<BatchRow>(
        db,
        "batches",
        IDBKeyRange.bound([sessionId, 0], [sessionId, Number.MAX_SAFE_INTEGER]),
      );
    },
    async putBatch(row) {
      const db = await opened;
      await idbPut(db, "batches", row);
    },
    async getSnapshot(sessionId) {
      const db = await opened;
      return idbGet<SnapshotRow>(db, "snapshots", sessionId);
    },
    async putSnapshot(row) {
      const db = await opened;
      await idbPut(db, "snapshots", row);
    },
    async getLastSessionId() {
      const db = await opened;
      const row = await idbGet<{ key: string; value: string }>(db, "meta", "lastSessionId");
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

function createArtifactStore(
  dbName: string,
  maxBlobBytes: number,
  maxBlobs: number,
  maxTotalBlobBytes: number,
  maxArtifactOwners: number,
  artifactOrphanGraceMs: number,
  maxArtifactGcBatch: number,
): HostArtifactStore {
  const opened = openExperimentalDb(dbName);
  return {
    async stagePut(scope, content, _metadata, artifact, storageKey) {
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
      const key = String(storageKey ?? "");
      const scopeJson = String(scope ?? "");
      const artifactJson = String(artifact ?? "");
      if (key.length === 0 || scopeJson.length === 0 || artifactJson.length === 0) {
        throwStoreError({
          code: "invalid_store_request",
          reasonCode: "artifact_identity_missing",
          message: "invalid store request: artifact_identity_missing",
        });
      }
      await mutateArtifactRows(db, (store, rows) => {
        const existing = rows.find((row) => row.storageKey === key);
        if (existing !== undefined) {
          const bytesMatch =
            existing.bytes.length === content.byteLength &&
            existing.bytes.every((value, index) => value === content[index]);
          if (
            existing.scope !== scopeJson ||
            existing.artifact !== artifactJson ||
            !bytesMatch
          ) {
            throwStoreError({
              code: "store_corruption",
              reasonCode: "artifact_identity_collision",
              message: "store corruption: artifact_identity_collision",
            });
          }
          return;
        }
        if (rows.length >= maxBlobs) {
          throwStoreError({
            code: "store_limit_exceeded",
            resource: "blobs",
            limit: maxBlobs,
            message: `store limit exceeded for blobs: ${maxBlobs}`,
          });
        }
        const totalBytes = rows.reduce((total, row) => total + row.byteLength, 0);
        if (totalBytes + content.byteLength > maxTotalBlobBytes) {
          throwStoreError({
            code: "store_limit_exceeded",
            resource: "total_blob_bytes",
            limit: maxTotalBlobBytes,
            message: `store limit exceeded for total_blob_bytes: ${maxTotalBlobBytes}`,
          });
        }
        store.put({
          storageKey: key,
          scope: scopeJson,
          artifact: artifactJson,
          bytes: Array.from(content),
          byteLength: content.byteLength,
          owners: [],
        });
      });
    },
    async get(scope, artifact, storageKey) {
      const db = await opened;
      await readSchemaVersion(db);
      const row = await idbGet<{
        storageKey: string;
        scope: string;
        artifact: string;
        bytes: number[];
      }>(db, "artifacts", String(storageKey));
      if (row === undefined) {
        throwStoreError({
          code: "invalid_store_request",
          reasonCode: "artifact_uncorrelated",
          message: "invalid store request: artifact_uncorrelated",
        });
      }
      if (row.scope !== String(scope) || row.artifact !== String(artifact)) {
        throwStoreError({
          code: "store_corruption",
          reasonCode: "artifact_scope_or_reference_mismatch",
          message: "store corruption: artifact_scope_or_reference_mismatch",
        });
      }
      return Uint8Array.from(row.bytes);
    },
    async getByBlob(scope, blob) {
      const db = await opened;
      await readSchemaVersion(db);
      const scopeJson = String(scope ?? "");
      const blobJson = String(blob ?? "");
      const rows = await idbGetAll<ArtifactRow>(db, "artifacts");
      for (const row of rows) {
        if (row.scope !== scopeJson) continue;
        let artifact: { blob?: unknown };
        try {
          artifact = JSON.parse(row.artifact) as { blob?: unknown };
        } catch {
          throwStoreError({
            code: "store_corruption",
            reasonCode: "artifact_reference_invalid",
            message: "store corruption: artifact_reference_invalid",
          });
        }
        if (JSON.stringify(artifact.blob) === blobJson) {
          return [row.artifact, Uint8Array.from(row.bytes)];
        }
      }
      throwStoreError({
        code: "invalid_store_request",
        reasonCode: "artifact_uncorrelated",
        message: "invalid store request: artifact_uncorrelated",
      });
    },
    async pin(scope, artifact, storageKey, owner) {
      const db = await opened;
      await readSchemaVersion(db);
      await mutateArtifactRows(db, (store, rows) => {
        const row = requireArtifactRow(rows, scope, artifact, storageKey);
        const ownerId = String(owner ?? "");
        if (ownerId.length === 0) {
          throwStoreError({
            code: "invalid_store_request",
            reasonCode: "artifact_owner_missing",
            message: "invalid store request: artifact_owner_missing",
          });
        }
        if (!row.owners.includes(ownerId) && row.owners.length >= maxArtifactOwners) {
          throwStoreError({
            code: "store_limit_exceeded",
            resource: "artifact_owners",
            limit: maxArtifactOwners,
            message: `store limit exceeded for artifact_owners: ${maxArtifactOwners}`,
          });
        }
        if (!row.owners.includes(ownerId)) row.owners.push(ownerId);
        delete row.unreferencedSince;
        store.put(row);
      });
    },
    async unpin(scope, artifact, storageKey, owner, nowUnixMs) {
      const db = await opened;
      await readSchemaVersion(db);
      await mutateArtifactRows(db, (store, rows) => {
        const row = requireArtifactRow(rows, scope, artifact, storageKey);
        const ownerId = String(owner);
        const nextOwners = row.owners.filter((value) => value !== ownerId);
        if (nextOwners.length !== row.owners.length) {
          row.owners = nextOwners;
          if (nextOwners.length === 0) row.unreferencedSince = Number(nowUnixMs);
          store.put(row);
        }
      });
    },
    async collectOrphans(scope, nowUnixMs, limit) {
      const db = await opened;
      await readSchemaVersion(db);
      return mutateArtifactRows(db, (store, rows) => {
        const scopeJson = String(scope);
        const now = Number(nowUnixMs);
        const boundedLimit = Math.min(Number(limit), maxArtifactGcBatch);
        let examined = 0;
        let deleted = 0;
        let bytesDeleted = 0;
        for (const row of rows) {
          if (examined >= boundedLimit) break;
          if (row.scope !== scopeJson) continue;
          examined += 1;
          if (row.owners.length > 0) continue;
          if (row.unreferencedSince === undefined) {
            row.unreferencedSince = now;
            store.put(row);
            continue;
          }
          if (now - row.unreferencedSince >= artifactOrphanGraceMs) {
            store.delete(row.storageKey);
            deleted += 1;
            bytesDeleted += row.byteLength;
          }
        }
        return { examined, deleted, bytes_deleted: bytesDeleted };
      });
    },
  };
}

function requireArtifactRow(
  rows: ArtifactRow[],
  scope: unknown,
  artifact: unknown,
  storageKey: unknown,
): ArtifactRow {
  const row = rows.find((candidate) => candidate.storageKey === String(storageKey));
  if (row === undefined) {
    throwStoreError({
      code: "invalid_store_request",
      reasonCode: "artifact_uncorrelated",
      message: "invalid store request: artifact_uncorrelated",
    });
  }
  if (row.scope !== String(scope) || row.artifact !== String(artifact)) {
    throwStoreError({
      code: "store_corruption",
      reasonCode: "artifact_scope_or_reference_mismatch",
      message: "store corruption: artifact_scope_or_reference_mismatch",
    });
  }
  return row;
}

function openExperimentalDb(dbName: string): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    let request: IDBOpenDBRequest;
    try {
      request = indexedDB.open(dbName, INDEXED_DB_SCHEMA_VERSION);
    } catch (error) {
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
      if (event.oldVersion < 2 && db.objectStoreNames.contains("artifacts")) {
        db.deleteObjectStore("artifacts");
      }
      if (!db.objectStoreNames.contains("artifacts")) {
        db.createObjectStore("artifacts", { keyPath: "storageKey" });
      }
      request.transaction?.objectStore("meta").put({
        key: "schemaVersion",
        value: INDEXED_DB_SCHEMA_VERSION,
      });
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

async function readSchemaVersion(db: IDBDatabase): Promise<number> {
  const row = await idbGet<{ key: string; value: number }>(db, "meta", "schemaVersion");
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

function schemaUnsupported(): FinstackError {
  const error = new FinstackError("store integrity failure: journal_schema_unsupported", {
    code: "store_integrity_failure",
    retryable: false,
  });
  Object.assign(error, { reasonCode: "journal_schema_unsupported" });
  return error;
}

function asSchemaError(error: unknown): FinstackError {
  if (error instanceof DOMException && error.name === "VersionError") {
    return schemaUnsupported();
  }
  if (error instanceof FinstackError) {
    return error;
  }
  return schemaUnsupported();
}

function deleteDatabase(dbName: string): Promise<void> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.deleteDatabase(dbName);
    request.onsuccess = () => resolve();
    request.onerror = () => reject(request.error ?? new Error("indexeddb delete failed"));
    request.onblocked = () => resolve();
  });
}

function idbGet<T>(db: IDBDatabase, store: string, key: IDBValidKey): Promise<T | undefined> {
  return new Promise((resolve, reject) => {
    const request = db.transaction(store, "readonly").objectStore(store).get(key);
    request.onsuccess = () => resolve(request.result as T | undefined);
    request.onerror = () => reject(request.error ?? new Error("indexeddb get failed"));
  });
}

function idbGetFromIndex<T>(
  db: IDBDatabase,
  store: string,
  index: string,
  key: IDBValidKey,
): Promise<T | undefined> {
  return new Promise((resolve, reject) => {
    const request = db
      .transaction(store, "readonly")
      .objectStore(store)
      .index(index)
      .get(key);
    request.onsuccess = () => resolve(request.result as T | undefined);
    request.onerror = () => reject(request.error ?? new Error("indexeddb index get failed"));
  });
}

function idbGetAll<T>(db: IDBDatabase, store: string): Promise<T[]> {
  return new Promise((resolve, reject) => {
    const request = db.transaction(store, "readonly").objectStore(store).getAll();
    request.onsuccess = () => resolve((request.result as T[] | undefined) ?? []);
    request.onerror = () => reject(request.error ?? new Error("indexeddb getAll failed"));
  });
}

function idbPut(db: IDBDatabase, store: string, value: unknown): Promise<void> {
  return idbWrite(db, [store], (txn) => {
    txn.objectStore(store).put(value);
  });
}

function mutateArtifactRows<T>(
  db: IDBDatabase,
  mutate: (store: IDBObjectStore, rows: ArtifactRow[]) => T,
): Promise<T> {
  return new Promise((resolve, reject) => {
    const txn = db.transaction("artifacts", "readwrite");
    const store = txn.objectStore("artifacts");
    const request = store.getAll();
    let result!: T;
    let operationError: unknown;
    request.onsuccess = () => {
      try {
        result = mutate(store, (request.result as ArtifactRow[] | undefined) ?? []);
      } catch (error) {
        operationError = error;
        txn.abort();
      }
    };
    request.onerror = () => {
      operationError = request.error ?? new Error("indexeddb artifact read failed");
      txn.abort();
    };
    txn.oncomplete = () => resolve(result);
    txn.onerror = () => reject(operationError ?? txn.error ?? new Error("indexeddb write failed"));
    txn.onabort = () =>
      reject(operationError ?? txn.error ?? new Error("indexeddb write aborted"));
  });
}

function idbWrite(
  db: IDBDatabase,
  stores: string[],
  mutate: (txn: IDBTransaction) => void,
): Promise<void> {
  return new Promise((resolve, reject) => {
    const txn = db.transaction(stores, "readwrite");
    txn.oncomplete = () => resolve();
    txn.onerror = () => reject(txn.error ?? new Error("indexeddb write failed"));
    txn.onabort = () => reject(txn.error ?? new Error("indexeddb write aborted"));
    mutate(txn);
  });
}

function idbGetAllInRange<T>(
  db: IDBDatabase,
  store: string,
  range: IDBKeyRange,
): Promise<T[]> {
  return new Promise((resolve, reject) => {
    const request = db.transaction(store, "readonly").objectStore(store).getAll(range);
    request.onsuccess = () => resolve((request.result as T[] | undefined) ?? []);
    request.onerror = () => reject(request.error ?? new Error("indexeddb range getAll failed"));
  });
}
