import { Agent, JsModel, buildMetadata, health, init } from "@finstack/ai";

/**
 * Type-only clean-install smoke for the staged npm tarball.
 *
 * Runtime Agent execution stays in the Playwright harness. This file proves
 * the packed declarations resolve without repository path mapping.
 */
export async function typecheckPublicSurface(): Promise<string> {
  await init();
  health();
  const metadata = buildMetadata();
  const model = new JsModel(
    {
      async request() {
        return { text: "ok", completion_id: "ts-alpha-1" };
      },
    },
    {
      component: "app.model.alpha",
      provider: "scripted",
      model: "scripted-model",
    },
  );
  const agent = await Agent.create({
    model,
    instruction: "Stable prefix.",
    capabilities: [
      {
        id: "app.capability.always",
        description: "Baseline guidance",
        instructions: ["Always instruction."],
        activation: "always",
      },
    ],
  });
  const catalog = agent.compactCapabilityCatalog();
  const result = await agent.run("hello");
  return `${metadata.version}:${catalog}:${result.text}:${result.trace[0] ?? ""}`;
}
