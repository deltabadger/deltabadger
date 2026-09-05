# frozen_string_literal: true

module BotApi
  module Bots
    # Brings an archived bot back, stopped. Bot.not_deleted is the enum's negative scope, so an
    # archived bot is still findable through it — which is what makes this reachable at all.
    class Unarchive
      include LifecycleSupport

      def self.call(user:, bot_id:) = new(user: user, bot_id: bot_id).call

      def initialize(user:, bot_id:)
        @user = user
        @bot_id = bot_id
      end

      def call
        bot = find_bot(@bot_id)
        return bot_not_found unless bot
        return Result.failure(:conflict, 'bot_not_archived', "Bot '#{bot.label}' is not archived.") unless bot.archived?
        return Result.success({ id: bot.id, label: bot.label, status: bot.status.to_s }) if bot.unarchive

        Result.failure(:validation_failed, 'bot_unarchive_failed',
                       "Failed to reactivate bot: #{bot.errors.full_messages.join(', ')}")
      end
    end
  end
end
