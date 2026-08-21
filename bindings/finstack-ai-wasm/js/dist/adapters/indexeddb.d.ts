/**
 * Experimental same-origin IndexedDB journal and artifact batteries.
 *
 * Persistence is origin-scoped and not crash-durable. `health().detail` stays
 * `js_indexeddb_experimental` and does not claim crash durability.
 * Schema version 2 stores exact scoped artifact references.
 */
import type { HostArtifactStore, HostJournalStore } from "../host.js";
/** Default experimental database name. */
export declare const INDEXED_DB_NAME = "finstack-ai-experimental";
/** Provisional IndexedDB schema version. */
export declare const INDEXED_DB_SCHEMA_VERSION = 2;
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
export declare function createIndexedDbJournalStore(options?: IndexedDbStoreOptions): HostJournalStore;
/**
 * Create an in-memory host journal that implements the same CAS contract.
 *
 * Used by in-page proofs. This is not durable and is not IndexedDB.
 *
 * @param options - Resource ceilings.
 * @returns A Map-backed host journal.
 */
export declare function createMapJournalStore(options?: IndexedDbStoreOptions): HostJournalStore;
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
export declare function createIndexedDbArtifactStore(options?: IndexedDbStoreOptions): HostArtifactStore;
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
export declare function deleteIndexedDbStores(options?: Pick<IndexedDbStoreOptions, "dbName">): Promise<void>;
//# sourceMappingURL=indexeddb.d.ts.map