module Utilities
  module Image
    def self.extract_dominant_colors(image_path, quantity = 5, threshold = 0.01)
      image = MiniMagick::Image.new(image_path)

      # Get image histogram
      result = begin
        MiniMagick::Tool::Magick.new do |convert|
          convert << image.path
          convert << '-format' << '%c'
          convert << '-colors' << quantity.to_s
          convert << '-depth' << '8'
          convert << '-alpha' << 'on'
          convert << 'histogram:info:'
          convert.call
        end
      rescue MiniMagick::Error
        # handle Imagemagick version <7
        # TODO: remove this rescue block once all servers are running ImageMagick >=7 (quick test: `magick --version`)
        image.run_command('convert', image.path, '-format', '%c', '-colors', quantity, '-depth', 8, 'histogram:info:')
      end

      # Extract colors and frequencies from result
      frequencies = result.scan(/([0-9]+):/).flatten.map(&:to_f)
      hex_values = result.scan(/(\#[0-9ABCDEF]{6,8})/).flatten
      total_frequencies = frequencies.reduce(:+).to_f

      # Create frequency/color pairs [frequency, hex],
      # sort by frequency,
      # ignore fully transparent colours
      # select items over frequency threshold (1% by default),
      # extract hex values,
      # return desired quantity
      frequencies
        .map.with_index { |f, i| [f / total_frequencies, hex_values[i]] }
        .sort           { |a, b| b[0] - a[0] }
        # .reject         { |r| r[1].size == 9 && r[1].end_with?('FF') }
        .select         { |r| r[0] > threshold }
        .map            { |r| r[1][0..6] }
        .slice(0, quantity)
    end

    def self.most_vivid_color(hex_colors)
      return nil if hex_colors.empty?

      # Convert each hex color to HSL and calculate vividness
      vividness_scores = hex_colors.map do |hex|
        # Convert hex to RGB
        hex = hex.gsub('#', '')
        r = hex[0..1].to_i(16) / 255.0
        g = hex[2..3].to_i(16) / 255.0
        b = hex[4..5].to_i(16) / 255.0

        # Find min and max for lightness calculation
        c_max = [r, g, b].max
        c_min = [r, g, b].min
        delta = c_max - c_min

        # Calculate lightness
        lightness = (c_max + c_min) / 2.0

        # Calculate saturation
        saturation = if delta.zero?
                       0.0
                     else
                       delta / (1.0 - (2.0 * lightness - 1.0).abs)
                     end

        # Define vividness as a combination of saturation and lightness
        # You can adjust the formula based on what "vivid" means to you
        # vividness = saturation * 0.7 + lightness * 0.3
        vividness = saturation

        # Give low score to near-black, near-white, or low-saturation colors
        next [hex, 0.0] if saturation < 0.2  # Skip low-saturation (grayish) colors
        next [hex, 0.0] if lightness < 0.1   # Skip near-black colors
        next [hex, 0.0] if lightness > 0.9   # Skip near-white colors

        [hex, vividness]
      end

      # Find the color with the highest vividness score
      most_vivid = vividness_scores.max_by { |pair| pair[1] }

      most_vivid ? "##{most_vivid[0]}" : nil
    end
  end
end
