module MetaTagsHelper
  def canonical_url_for_static_page
    return unless I18n.locale != :en

    tag(:link, rel: 'canonical', href: url_for(locale: :en, only_path: false))
  end

  def meta_description_for_static_page(page_key)
    t("meta.descriptions.#{page_key}", default: t('meta.descriptions.default'))
  end

  def page_title_for_static_page(page_key)
    t("meta.titles.#{page_key}", default: t('meta.title'))
  end
end
