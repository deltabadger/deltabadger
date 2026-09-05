# frozen_string_literal: true

module BotApi
  module Bots
    # Stops the bot and takes it off the dashboard, keeping its history. Reversible.
    class Archive
      include LifecycleSupport

      def self.call(user:, bot_id:) = new(user: user, bot_id: bot_id).call

      def initialize(user:, bot_id:)
        @user = user
        @bot_id = bot_id
      end

      def call
        bot = find_bot(@bot_id)
        return bot_not_found unless bot
        return Result.failure(:conflict, 'bot_archived', "Bot '#{bot.label}' is already archived.") if bot.archived?
        return Result.success({ id: bot.id, label: bot.label, status: bot.status.to_s }) if bot.archive

        Result.failure(:validation_failed, 'bot_archive_failed',
                       "Failed to archive bot: #{bot.errors.full_messages.join(', ')}")
      end
    end
  end
end
