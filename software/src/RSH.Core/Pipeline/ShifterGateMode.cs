namespace RSH.Core.Pipeline;

/// <summary>
/// Shifter module ka gate mode (Phase F WS5, Plan v2.3 §4 — Shifter
/// module: sequential / H-pattern).
/// <list type="bullet">
/// <item><see cref="Sequential"/> (default): do buttons — gear up / gear down.
/// Stream buttons bit 0 = up, bit 1 = down; pipeline unhein vJoy buttons
/// ki tarah aage bhejta hai (v1 legacy remap / v2 absolute).</item>
/// <item><see cref="HPattern"/>: 6 gear switches. Firmware stream buttons
/// bit 5..10 mein gear bhejta hai (bit 5 = gear 1 … bit 10 = gear 6 —
/// D1 absolute button numbering barqarar: accel/brake/horn bits 2/3/4
/// dono modes mein wahi rehte hain). Pipeline in bits se
/// <see cref="RSH.Core.Output.ProcessedState.Gear"/> nikalta hai
/// (0 = neutral, 1..6 = gear) aur gear bits ko ButtonsMask mein nahi
/// bhejta — wo gear state hain, momentary buttons nahi. Ek se zyada
/// gear bits (ya koi bhi nahi) = neutral (firmware
/// <c>rsh_shifter_hpattern_gear</c> wala hi usool).</item>
/// </list>
/// Gate mode <see cref="ModuleTuning"/> mein rehta hai (PipelinePlan) —
/// <c>WheelProfile</c> mein nahi (W2 ka hai). Firmware ko mode
/// SET_CFG config blob se milta hai (blob type 11 SHIFTER record,
/// params[0] = mode — PROTOCOL-V2.1.md §3.6).
/// v1 legacy stream mein H-pattern ka koi wire slot nahi — v1 + HPattern
/// par Gear hamesha 0 rehta hai (sequential fallback, §3.6 note).
/// </summary>
public enum ShifterGateMode
{
    Sequential = 0,
    HPattern = 1,
}
