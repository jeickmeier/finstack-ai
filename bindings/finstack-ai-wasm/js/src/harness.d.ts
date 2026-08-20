import type {
  Agent,
  ApprovalGrantMode,
  BuildMetadata,
  FinstackError,
  HostArtifactStore,
  HostClock,
  HostContextProvider,
  HostJournalStore,
  SessionInspectSnapshot,
  HostMiddleware,
  HostModel,
  HostObserver,
  HostRandomSource,
  HostToolset,
  JsArtifactStore,
  JsClock,
  JsContextProvider,
  JsContextProviderOptions,
  JsJournalStore,
  JsJournalStoreOptions,
  JsMiddleware,
  JsMiddlewareOptions,
  JsModel,
  JsModelOptions,
  JsObserver,
  JsObserverOptions,
  JsRandomSource,
  JsToolset,
  JsToolsetOptions,
  ParsedDocument,
  PrebetaKind,
} from "./index.ts";
import type { OpenAIOptions } from "./adapters/openai.ts";
import type {
  createIndexedDbArtifactStore,
  createIndexedDbJournalStore,
  createMapJournalStore,
  deleteIndexedDbStores,
} from "./adapters/indexeddb.ts";
import type { WorkerClient, WorkerConnectOptions } from "./worker.ts";

type DriveResult = {
  ok: boolean;
  code?: string;
  category?: string;
  text?: string;
  completion_id?: string;
  tool_calls?: unknown[];
  output?: unknown;
  is_error?: boolean;
  ready?: boolean;
  durable?: boolean;
  detail?: string;
};

declare global {
  interface Window {
    finstackReady: Promise<void>;
    finstackTest: {
      health: () => string;
      buildMetadata: () => BuildMetadata;
      Agent: typeof Agent;
      ApprovalGrantMode: typeof ApprovalGrantMode;
      FinstackError: typeof FinstackError;
      compilePortProxies: () => void;
      runNoopTrace: () => string;
      JsModel: new (adapter: HostModel, options: JsModelOptions) => JsModel;
      JsToolset: new (adapter: HostToolset, options: JsToolsetOptions) => JsToolset;
      JsContextProvider: new (
        adapter: HostContextProvider,
        options: JsContextProviderOptions,
      ) => JsContextProvider;
      JsMiddleware: new (
        adapter: HostMiddleware,
        options: JsMiddlewareOptions,
      ) => JsMiddleware;
      JsObserver: new (adapter: HostObserver, options: JsObserverOptions) => JsObserver;
      JsJournalStore: new (
        adapter: HostJournalStore,
        options?: JsJournalStoreOptions,
      ) => JsJournalStore;
      JsClock: new (adapter: HostClock) => JsClock;
      JsRandomSource: new (adapter: HostRandomSource) => JsRandomSource;
      JsArtifactStore: new (adapter: HostArtifactStore) => JsArtifactStore;
      createMemoryJournalStore: () => HostJournalStore;
      createMemoryArtifactStore: () => HostArtifactStore;
      createMapJournalStore: typeof createMapJournalStore;
      createIndexedDbJournalStore: typeof createIndexedDbJournalStore;
      createIndexedDbArtifactStore: typeof createIndexedDbArtifactStore;
      deleteIndexedDbStores: typeof deleteIndexedDbStores;
      inspectSession: (
        store: JsJournalStore,
        sessionId: string,
      ) => Promise<SessionInspectSnapshot>;
      createHostClock: (now?: () => number) => HostClock;
      createHostRandomSource: () => HostRandomSource;
      createOpenAIModel: (options?: OpenAIOptions) => HostModel;
      openaiDefaultBaseUrl: string;
      journalKnownAnswer: (
        kind: "record_body" | "record_envelope",
        value: unknown,
      ) => { payload_digest: string; checksum?: string; cbor_hex: string };
      normalizePrebetaShape: (kind: PrebetaKind, value: unknown) => unknown;
      parseDocumentMarkdown: (data: Uint8Array, mediaType: string) => string;
      parseDocument: (data: Uint8Array, mediaType: string) => ParsedDocument;
      applyScriptedCoordinatorCommands: (
        commands: ReadonlyArray<{ kind: PrebetaKind; value: unknown }>,
      ) => { commands: Array<{ kind: string; status: string }> };
      driveScriptedModelRequest: (
        adapter: HostModel,
        options: JsModelOptions,
        signal?: AbortSignal,
      ) => Promise<DriveResult>;
      driveScriptedToolCall: (
        adapter: HostToolset,
        options: JsToolsetOptions,
        signal?: AbortSignal,
      ) => Promise<DriveResult>;
      driveScriptedJournalHealth: (
        adapter: HostJournalStore,
        options?: JsJournalStoreOptions,
      ) => Promise<DriveResult>;
      connectWorker: (
        worker: Worker,
        options?: WorkerConnectOptions,
      ) => Promise<WorkerClient>;
      createAgentWorker: () => Worker;
      uiTicks: () => number;
    };
    finstackWorkerReady: Promise<void>;
    finstackWorker: {
      connectWorker: (
        worker: Worker,
        options?: WorkerConnectOptions,
      ) => Promise<WorkerClient>;
      createAgentWorker: () => Worker;
      uiTicks: () => number;
    };
  }
}

export {};
