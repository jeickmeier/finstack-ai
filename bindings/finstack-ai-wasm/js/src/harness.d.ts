import type {
  Agent,
  BuildMetadata,
  FinstackError,
  HostArtifactStore,
  HostClock,
  HostContextProvider,
  HostJournalStore,
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
  PrebetaKind,
} from "./index.ts";
import type {
  OpenAICompatibleOptions,
} from "./adapters/openai-compatible.ts";

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
      createHostClock: (now?: () => number) => HostClock;
      createHostRandomSource: () => HostRandomSource;
      createOpenAICompatibleModel: (options?: OpenAICompatibleOptions) => HostModel;
      openaiCompatibleDefaultBaseUrl: string;
      normalizePrebetaShape: (kind: PrebetaKind, value: unknown) => unknown;
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
    };
  }
}

export {};
