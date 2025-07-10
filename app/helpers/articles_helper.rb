module ArticlesHelper
  def article_json_ld(article)
    {
      "@context": 'https://schema.org',
      "@type": 'Article',
      "headline": j(article.plain_title),
      "alternativeHeadline": j(article.subtitle.presence),
      "description": j(article.excerpt),
      "datePublished": article.published_at&.iso8601,
      "dateModified": article.updated_at&.iso8601,
      "author": if article.author.present?
                  {
                    "@type": 'Person',
                    "name": j(article.author.name),
                    "url": article.author.url.presence
                  }
                end,
      "publisher": {
        "@type": 'Organization',
        "name": 'Deltabadger',
        "logo": {
          "@type": 'ImageObject',
          "url": asset_url('app-day.png')
        }
      },
      "image": if article.thumbnail_path.present?
                 {
                   "@type": 'ImageObject',
                   "url": asset_url(article.thumbnail_path)
                 }
               end,
      "timeRequired": article.reading_time_minutes.present? ? "PT#{article.reading_time_minutes}M" : nil,
      "inLanguage": article.locale,
      "url": article_url(article, locale: article.locale),
      "isAccessibleForFree": !article.paywalled?,
      "hasPart": if article.paywalled?
                   [{
                     "@type": 'WebPageElement',
                     "isAccessibleForFree": true,
                     "cssSelector": '.article-body'
                   }]
                 end
    }.compact.to_json
  end
end
