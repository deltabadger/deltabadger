module ApplicationHelper
  def html_class
    classes = []
    classes << 'view--logged-in' if user_signed_in?
    classes << "view--#{controller_name}-#{action_name}"
    classes.join(' ')
  end

  def render_turbo_stream_flash_messages
    turbo_stream.prepend 'flash', partial: 'layouts/flash'
  end

  def protect_ticker_text_contrast(bg_color)
    # Convert the background color to RGB
    r = bg_color[1..2].hex
    g = bg_color[3..4].hex
    b = bg_color[5..6].hex

    # Calculate the luminance 
    # Default:   0.299 * r + 0.587 * g + 0.114 * b
    luminance = (0.262 * r + 0.587 * g + 0.114 * b) / 255

    # Return black or white text based on luminance
    luminance > 0.659 ? "var(--ticker-text-dark)" : "#fff"
  end
end
