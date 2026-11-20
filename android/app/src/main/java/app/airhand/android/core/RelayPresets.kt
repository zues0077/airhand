package app.airhand.android.core

/**
 * 手机端"设置"里的中转快捷预设。和桌面端 SettingsDialog.tsx 的 PRESETS 保持一致。
 * 只有手机界面用；电视端保持上游原样，不显示这些按钮。
 */
data class RelayPreset(val key: String, val server: String, val fp: String)

object RelayPresets {
    val self = RelayPreset(
        "self",
        "airhand.fnosliu.de5.net:52141",
        "a27582a2522417fe6058e545592ec289fedcf67a0575909783441c897ecb2abf",
    )
    val public = RelayPreset(
        "public",
        "39.105.40.56:4433",
        "5c1e5ad85089bfa938b8b7f5008076c42ad6dfe3aee5e623300ef93d0efa8fbe",
    )
    val entries = listOf(self, public)
}
