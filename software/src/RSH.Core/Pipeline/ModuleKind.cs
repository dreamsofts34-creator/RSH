namespace RSH.Core.Pipeline;

/// <summary>
/// RSH-PLAN-v2.3 §4 ke 10 modules. Har module ki apni tuning + on/off
/// hoti hai; PipelinePlan har module ke liye ek entry rakhta hai.
/// </summary>
public enum ModuleKind
{
    Steering,
    Accelerator,
    Brake,
    Clutch,
    Handbrake,
    Shifter,
    Horn,
    Buttons,
    Encoders,
    LED,
}
