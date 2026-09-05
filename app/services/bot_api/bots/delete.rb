# frozen_string_literal: true

module BotApi
  module Bots
    # Soft: status becomes deleted and the history stays, exactly like the Delete modal. Any
    # status, like the modal — but through #delete, which also cancels a scheduled tick.
    class Delete
      include LifecycleSupport

      def self.call(user:, bot_id:) = new(user: user, bot_id: bot_id).call

      def initialize(user:, bot_id:)
        @user = user
        @bot_id = bot_id
      end

      def call
        bot = find_bot(@bot_id)
        return bot_not_found unless bot
        return Result.success({ id: bot.id, label: bot.label, status: 'deleted' }) if bot.delete

        Result.failure(:validation_failed, 'bot_delete_failed',
                       "Failed to delete bot: #{bot.errors.full_messages.join(', ')}")
      end
    end
  end
end
