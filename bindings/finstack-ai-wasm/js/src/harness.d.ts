import type { BuildMetadata } from "./index.ts";

declare global {
  interface Window {
    finstackReady: Promise<void>;
    finstackTest: {
      health: () => string;
      buildMetadata: () => BuildMetadata;
      compilePortProxies: () => void;
      runNoopTrace: () => string;
    };
  }
}

export {};
