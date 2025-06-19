module Bot::Dryable
  extend ActiveSupport::Concern

  included do
    decorators = Module.new do
      def api_key
        if Rails.configuration.dry_run
          user.api_keys.new(exchange_id: exchange_id, key_type: api_key_type, status: :correct)
        else
          super
        end
      end

      def notify_if_funds_are_low
        return if Rails.configuration.dry_run

        super
      end
    end

    prepend decorators
  end
end
