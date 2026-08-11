use finstack_ai::Agent;
use finstack_ai_provider_openai_compatible::OpenAiCompatibleProvider;
use finstack_ai_tools_calculator::CalculatorToolset;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let _public_types = (
        std::any::type_name::<Agent>(),
        std::any::type_name::<OpenAiCompatibleProvider>(),
        std::any::type_name::<CalculatorToolset>(),
    );
    let result = finstack_ai_native_examples::run_tool_loop().await?;
    assert_eq!(result, "five");
    println!("clean native user run: {result}");
    Ok(())
}
