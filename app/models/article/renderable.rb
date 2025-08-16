require 'kramdown'
require 'kramdown/parser/gfm'

module Article::Renderable
  extend ActiveSupport::Concern

  PAYWALL_START_MARKER = '<!-- PAYWALL -->'.freeze
  PAYWALL_END_MARKER = '<!-- /PAYWALL -->'.freeze

  def render_content(user: nil)
    if user&.can_access_full_articles?
      markdown_to_html(content)
    else
      render_content_with_inline_paywall
    end
  end

  private

  def render_content_with_inline_paywall
    return markdown_to_html(content) unless paywalled?

    # For section-based paywall, insert paywall UI where premium content should be
    if content.include?(PAYWALL_START_MARKER) && content.include?(PAYWALL_END_MARKER)
      render_section_based_content_with_paywall
    else
      # Legacy: render free content + paywall at the end
      markdown_to_html(free_content) + "\n\n<!-- INLINE_PAYWALL -->"
    end
  end

  def render_section_based_content_with_paywall
    result = content.dup

    # Replace each paywall section with a placeholder
    while result.include?(PAYWALL_START_MARKER) && result.include?(PAYWALL_END_MARKER)
      start_pos = result.index(PAYWALL_START_MARKER)
      end_pos = result.index(PAYWALL_END_MARKER, start_pos)

      break if end_pos.nil?

      # Replace the entire paywall section with placeholder
      paywall_placeholder = "\n\n<!-- INLINE_PAYWALL -->\n\n"
      result = result[0...start_pos] + paywall_placeholder + result[(end_pos + PAYWALL_END_MARKER.length)..-1]
    end

    markdown_to_html(result)
  end

  def markdown_to_html(text)
    return '' if text.blank?

    # Use kramdown with GitHub Flavored Markdown for full feature support
    # including tables, syntax highlighting, strikethrough, etc.
    kramdown_options = {
      input: 'GFM',                    # GitHub Flavored Markdown
      hard_wrap: true,                 # Convert single line breaks to <br>
      auto_ids: true,                  # Generate IDs for headers
      syntax_highlighter: nil,         # Disable syntax highlighting for now
      parse_block_html: true,          # Parse markdown inside HTML block elements
      parse_span_html: true,           # Parse markdown inside HTML span elements
      html_to_native: true,            # Convert HTML elements to native kramdown elements when possible
      smart_quotes: %w[apos apos quot quot], # Use straight quotes
      typographic_symbols: {
        hellip: '...',
        mdash: '---',
        ndash: '--',
        laquo: '<<',
        raquo: '>>'
      }
    }

    document = Kramdown::Document.new(text, kramdown_options)
    document.to_html.html_safe
  end
end
