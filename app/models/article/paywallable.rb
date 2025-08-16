module Article::Paywallable
  extend ActiveSupport::Concern

  PAYWALL_START_MARKER = '<!-- PAYWALL -->'.freeze
  PAYWALL_END_MARKER = '<!-- /PAYWALL -->'.freeze

  def paywalled?
    content.include?(PAYWALL_START_MARKER)
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
    # Check if we have both start and end markers (section-based paywall)
    if content.include?(PAYWALL_START_MARKER) && content.include?(PAYWALL_END_MARKER)
      split_section_based_paywall(content)
    else
      # Legacy behavior: split everything after the start marker
      split_legacy_paywall(content)
    end
  end

  def split_section_based_paywall(content)
    # Find all paywall sections and remove them from free content
    free_content = content.dup
    premium_sections = []

    # Process all paywall sections (in case there are multiple)
    while free_content.include?(PAYWALL_START_MARKER) && free_content.include?(PAYWALL_END_MARKER)
      start_pos = free_content.index(PAYWALL_START_MARKER)
      end_pos = free_content.index(PAYWALL_END_MARKER, start_pos)

      # If we can't find a matching end marker after the start marker, break
      break if end_pos.nil?

      # Extract the premium section (including markers)
      premium_section_start = start_pos + PAYWALL_START_MARKER.length
      premium_section = free_content[premium_section_start...end_pos].strip
      premium_sections << premium_section if premium_section.present?

      # Remove the entire paywall section (including markers) from free content
      free_content = free_content[0...start_pos] + free_content[(end_pos + PAYWALL_END_MARKER.length)..-1]
    end

    {
      free: free_content.strip,
      premium: premium_sections.join("\n\n")
    }
  end

  def split_legacy_paywall(content)
    parts = content.split(PAYWALL_START_MARKER, 2)

    {
      free: parts[0]&.strip || '',
      premium: parts[1]&.strip || ''
    }
  end
end
