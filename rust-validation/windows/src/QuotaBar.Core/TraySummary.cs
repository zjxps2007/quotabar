namespace QuotaBar.Core;

/// <summary>The tray is a single fixed-width icon. Summarize only accounts the user selected for it.</summary>
public sealed record TraySummary(string Text, bool UseLogo, bool Warning, string Tooltip)
{
    public static TraySummary From(DisplayState state) => NativeDisplay.Tray(state);
}
