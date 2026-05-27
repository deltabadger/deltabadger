# frozen_string_literal: true

module BotApi
  module Bots
    class Start
      def self.call(user:, bot_id:)
        new(user: user, bot_id: bot_id).call
      end

      def initialize(user:, bot_id:)
        @user = user
        @bot_id = bot_id
      end

      def call
        bot = @user.bots.not_deleted.find_by(id: @bot_id.to_i)
        return Result.failure(:not_found, 'bot_not_found', 'Bot not found.') unless bot

        if bot.working?
          return Result.failure(:conflict, 'bot_already_running',
                                "Bot '#{bot.label}' is already running (#{bot.status}).",
                                data: { id: bot.id, label: bot.label, status: bot.status.to_s })
        end

        start_fresh = bot.created?
        bot.set_missed_quote_amount
        if bot.start(start_fresh: start_fresh)
          Result.success({ id: bot.id, label: bot.label, status: bot.status.to_s })
        else
          Result.failure(:validation_failed, 'bot_start_failed',
                         "Failed to start bot '#{bot.label}': #{bot.errors.full_messages.join(', ')}")
        end
      end
    end
  end
end
