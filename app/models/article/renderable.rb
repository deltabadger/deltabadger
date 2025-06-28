require 'kramdown'
require 'kramdown/parser/gfm'

module Article::Renderable
  extend ActiveSupport::Concern

  def render_content(user: nil)
    rendered_content = markdown_to_html(content)

    if has_paywall? && !user_has_access?(user)
      split_content_at_paywall(rendered_content)[:free]
    else
      rendered_content
    end
  end

  def render_full_content
    markdown_to_html(content)
  end

  def render_excerpt
    return excerpt if excerpt.present?

    # Extract excerpt from content
    first_paragraph = content.split("\n\n").first
    markdown_to_html(first_paragraph) if first_paragraph.present?
  end

  private

  def markdown_to_html(text)
    return '' if text.blank?

    # Use kramdown with GitHub Flavored Markdown for full feature support
    # including tables, syntax highlighting, strikethrough, etc.
    kramdown_options = {
      input: 'GFM',                    # GitHub Flavored Markdown
      hard_wrap: false,                # Don't convert single line breaks to <br>
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
    document.to_html
  end
end
