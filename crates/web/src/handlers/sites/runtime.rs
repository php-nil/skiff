//! 运行环境扩展清单：`/ui/v1/runtimes/{id}` 抽屉里的可勾选项。

use crate::templates::ExtensionOptionView;

pub(crate) fn extension_options(
    enabled: &[String],
    version: &str,
    image: &str,
) -> Vec<ExtensionOptionView> {
    let base = skiff_app::sites::runtime_catalog::base_image_of(image);
    let mut out: Vec<ExtensionOptionView> =
        skiff_app::sites::runtime_catalog::extensions_for(version, base)
            .into_iter()
            .map(|entry| {
                let e = entry.def;
                // 不可用项已在 `extensions_for` 过滤，展示层不再灰显/占位；
                // 这里只保留依赖提示（Ok → 空）。
                let support_note = match &entry.support {
                    skiff_app::sites::runtime_catalog::ExtSupport::NeedsDeps(deps) => {
                        format!("安装时将临时安装 {} 并在完成后清理", deps.join(" / "))
                    }
                    _ => String::new(),
                };
                ExtensionOptionView {
                    key: e.key.to_string(),
                    label: e.label.to_string(),
                    description: e.description.to_string(),
                    // 镜像内置项始终视为已启用（无需再编译）。
                    enabled: e.builtin || enabled.iter().any(|k| k == e.key),
                    default_on: e.default_on,
                    builtin: e.builtin,
                    category: e.category.to_string(),
                    source: crate::labels::ext_source(e.source).to_string(),
                    hot: e.hot,
                    heavy: skiff_app::sites::runtime_catalog::extension_is_heavy(e.key),
                    estimated_time: skiff_app::sites::runtime_catalog::extension_estimated_time(
                        e.key,
                    )
                    .to_string(),
                    support_note,
                }
            })
            .collect();
    // 运行环境里按名字装过的 PECL 扩展也要出现在清单里（否则无法卸载）。
    for token in enabled {
        if skiff_app::sites::runtime_catalog::php_extension(token).is_none()
            && skiff_app::sites::runtime_catalog::parse_pecl_token(token).is_some()
        {
            out.push(ExtensionOptionView {
                key: token.clone(),
                label: skiff_app::sites::runtime_catalog::extension_label(token),
                description: "通过 PECL 按名字安装".to_string(),
                enabled: true,
                default_on: false,
                builtin: false,
                category: skiff_app::sites::runtime_catalog::CATEGORY_OTHER.to_string(),
                source: crate::labels::ext_source(
                    skiff_app::sites::runtime_catalog::ExtSource::Pecl,
                )
                .to_string(),
                hot: true,
                heavy: skiff_app::sites::runtime_catalog::extension_is_heavy(token),
                estimated_time: skiff_app::sites::runtime_catalog::extension_estimated_time(token)
                    .to_string(),
                support_note: String::new(),
            });
        }
    }
    out
}
