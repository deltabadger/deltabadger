require 'kramdown'
require 'kramdown/parser/gfm'

module Article::Renderable
  extend ActiveSupport::Concern

  def render_content(user: nil)
    if user&.can_access_full_articles?
      markdown_to_html(content)
    else
      markdown_to_html(free_content)
    end
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
    document.to_html.html_safe
  end
end
