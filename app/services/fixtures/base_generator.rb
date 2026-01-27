module Fixtures
  class BaseGenerator
    def initialize
      @fixtures_dir = Rails.root.join("db", "fixtures")
    end

    protected

    def write_json_file(filename, data, metadata: {})
      file_path = @fixtures_dir.join(filename)
      FileUtils.mkdir_p(file_path.dirname)

      output = {
        metadata: {
          generated_at: Time.current.iso8601,
          version: "1.0",
          generator: self.class.name
        }.merge(metadata),
        data: data
      }

      File.write(file_path, JSON.pretty_generate(output))
      Rails.logger.info "Generated fixture: #{file_path} (#{data.size} records)"
      file_path
    end

    def log_info(message)
      Rails.logger.info "[#{self.class.name}] #{message}"
    end

    def log_error(message)
      Rails.logger.error "[#{self.class.name}] #{message}"
    end

    def require_coingecko!
      unless AppConfig.coingecko_api_key.present?
        raise "CoinGecko API key not configured. Set COINGECKO_API_KEY environment variable."
      end
    end
  end
end
