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

  def ensure_contrast(bg_color, contrast_ratio)
    # Convert the background color to RGB
    r = bg_color[1..2].hex / 255.0
    g = bg_color[3..4].hex / 255.0
    b = bg_color[5..6].hex / 255.0

    # Calculate the relative luminance
    r = r <= 0.03928 ? r / 12.92 : ((r + 0.055) / 1.055) ** 2.4
    g = g <= 0.03928 ? g / 12.92 : ((g + 0.055) / 1.055) ** 2.4
    b = b <= 0.03928 ? b / 12.92 : ((b + 0.055) / 1.055) ** 2.4

    luminance = 0.2126 * r + 0.7152 * g + 0.0722 * b

    # Calculate contrast ratio
    def contrast_ratio(l1, l2)
      (l1 + 0.05) / (l2 + 0.05)
    end

    white_luminance = 1.0
    black_luminance = 0.0

    white_contrast = contrast_ratio(white_luminance, luminance)
    black_contrast = contrast_ratio(luminance, black_luminance)

    # Ensure contrast ratio
    white_contrast >= contrast_ratio ? "#fff" : "var(--text-dark)"
  end
end
