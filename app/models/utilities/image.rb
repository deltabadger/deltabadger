module Utilities
  module Image
    # The image URL is a field in the market-data provider's JSON — upstream text, not
    # something this app chose. ImageMagick treats its input argument as a specification
    # rather than a filename: a remote URL makes IT fetch the bytes, from inside the
    # container network, and a `coder:` prefix changes what it does with them. So the URL is
    # checked here, the bytes are fetched with the app's own HTTP stack, and only a path on
    # this filesystem is ever handed to convert.
    MAX_BYTES = 5_000_000
    TIMEOUT = 5

    class SizeLimitExceeded < StandardError
    end

    ALLOWED_HOSTS = %w[coin-images.coingecko.com assets.coingecko.com].freeze

    # Content sniffed from the first bytes rather than trusted from the URL or the
    # Content-Type header, and anything unrecognised is refused — so a payload dressed as an
    # image (an MSL script, an SVG carrying an external entity) never reaches convert.
    MAGIC = {
      "\x89PNG\r\n\x1A\n".b => 'png',
      "\xFF\xD8\xFF".b => 'jpg',
      'GIF87a'.b => 'gif',
      'GIF89a'.b => 'gif'
    }.freeze

    def self.allowed_source?(url)
      uri = URI.parse(url.to_s)

      uri.is_a?(URI::HTTPS) && uri.host.present? && allowed_hosts.include?(uri.host.downcase)
    rescue URI::Error, ArgumentError
      false
    end

    def self.allowed_hosts
      configured = begin
        URI.parse(MarketDataSettings.deltabadger_public_url.to_s).host
      rescue URI::Error
        nil
      end

      ALLOWED_HOSTS + [configured&.downcase].compact
    end

    def self.dominant_colors_from_url(url, quantity = 5, threshold = 0.01)
      return nil unless allowed_source?(url)

      bytes = fetch(url)
      coder = bytes && MAGIC.find { |magic, _| bytes.start_with?(magic) }&.last
      return nil if coder.nil?

      Tempfile.create(['asset-image', ".#{coder}"], binmode: true) do |file|
        file.write(bytes)
        file.flush
        extract_dominant_colors("#{coder}:#{file.path}", quantity, threshold)
      end
    end

    # Counted while the body arrives, not after. Checking afterwards still lets a peer that
    # answers with an unbounded stream have the whole of it buffered first, which is the
    # allocation the cap exists to prevent — so the request is abandoned the moment the
    # running total passes it.
    def self.fetch(url)
      buffer = +''
      overflowed = false

      connection = Faraday.new(request: { timeout: TIMEOUT, open_timeout: TIMEOUT }) do |f|
        f.response :raise_error
      end

      connection.get(url) do |req|
        req.options.on_data = proc do |chunk, _overall|
          next if overflowed

          buffer << chunk
          if buffer.bytesize > MAX_BYTES
            overflowed = true
            buffer.clear
            raise SizeLimitExceeded
          end
        end
      end

      overflowed ? nil : buffer
    rescue SizeLimitExceeded, Faraday::Error, StandardError
      nil
    end

    def self.extract_dominant_colors(image_path, quantity = 5, threshold = 0.01)
      # Get image histogram using the convert tool
      result = MiniMagick.convert do |convert|
        convert << image_path
        convert << '-format' << '%c'
        convert << '-colors' << quantity.to_s
        convert << '-depth' << '8'
        convert << '-alpha' << 'on'
        convert << 'histogram:info:'
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
        .sort { |a, b| b[0] - a[0] }
        .select { |r| r[0] > threshold }
        .map { |r| r[1][0..6] }
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
                       delta / (1.0 - ((2.0 * lightness) - 1.0).abs)
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
