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

  def hex_to_rgb(hex)
    hex = hex.gsub('#', '')
    r = hex[0..1].hex
    g = hex[2..3].hex
    b = hex[4..5].hex
    [r, g, b]
  end

  def rgb_to_hex(r, g, b)
    '#' + [r, g, b].map { |x| x.to_s(16).rjust(2, '0') }.join
  end

  def calculate_luminance(r, g, b)
    r /= 255.0
    g /= 255.0
    b /= 255.0

    r = r <= 0.03928 ? r / 12.92 : ((r + 0.055) / 1.055)**2.4
    g = g <= 0.03928 ? g / 12.92 : ((g + 0.055) / 1.055)**2.4
    b = b <= 0.03928 ? b / 12.92 : ((b + 0.055) / 1.055)**2.4

    0.2126 * r + 0.7152 * g + 0.0722 * b
  end

  def lighten_color(r, g, b, percentage)
    [(r + (255 - r) * percentage).round, (g + (255 - g) * percentage).round, (b + (255 - b) * percentage).round]
  end

  def darken_color(r, g, b, percentage)
    [(r * (1 - percentage)).round, (g * (1 - percentage)).round, (b * (1 - percentage)).round]
  end

  def ensure_contrast(color)
    return color if color.blank?

    r, g, b = hex_to_rgb(color)
    luminance = calculate_luminance(r, g, b)

    if luminance > 0.7
      r, g, b = darken_color(r, g, b, 0.2)
    elsif luminance < 0.3 && luminance > 0.04
      r, g, b = lighten_color(r, g, b, 0.1)
    elsif luminance <= 0.04
      r, g, b = lighten_color(r, g, b, 0.4)
    end

    rgb_to_hex(r, g, b)
  end

  def ensure_contrast_text(bg_color, contrast_ratio)
    return if bg_color.blank?

    r, g, b = hex_to_rgb(bg_color)
    luminance = calculate_luminance(r, g, b)

    white_luminance = 1.0
    black_luminance = 0.0

    white_contrast = (white_luminance + 0.05) / (luminance + 0.05)
    black_contrast = (luminance + 0.05) / (black_luminance + 0.05)

    white_contrast >= contrast_ratio ? '#fff' : 'var(--text-dark)'
  end
end
