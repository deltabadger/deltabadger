module Bot::Dryable
  extend ActiveSupport::Concern

  included do
    decorators = Module.new do
      def api_key
        if Rails.configuration.dry_run
          user.api_keys.trading.new(exchange_id: exchange_id, status: :correct)
        else
          super
        end
      end
    end

    prepend decorators
  end
end
