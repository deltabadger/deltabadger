module Bot::Dryable
  extend ActiveSupport::Concern

  included do
    decorators = Module.new do
      def api_key
        if dry_run?
          user.api_keys.new(exchange_id: exchange_id, key_type: api_key_type, status: :correct)
        else
          super
        end
      end
    end

    prepend decorators

    private

    def dry_run?
      Rails.configuration.dry_run
    end
  end
end
