module Article::Paywallable
  extend ActiveSupport::Concern

  PAYWALL_MARKER = '<!-- PAYWALL -->'.freeze

  def paywalled?
    content.include?(PAYWALL_MARKER)
  end

  def free_content
    return content unless paywalled?

    split_content_at_paywall(content)[:free]
  end

  def premium_content
    return '' unless paywalled?

    split_content_at_paywall(content)[:premium]
  end

  private

  def split_content_at_paywall(content)
    parts = content.split(PAYWALL_MARKER, 2)

    {
      free: parts[0]&.strip || '',
      premium: parts[1]&.strip || ''
    }
  end
end
