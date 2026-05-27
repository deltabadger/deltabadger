# frozen_string_literal: true

module BotApi
  module Bots
    class Stop
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

        unless bot.working?
          return Result.failure(:conflict, 'bot_not_running',
                                "Bot '#{bot.label}' is not running (#{bot.status}).",
                                data: { id: bot.id, label: bot.label, status: bot.status.to_s })
        end

        bot.set_missed_quote_amount
        if bot.stop
          Result.success({ id: bot.id, label: bot.label, status: bot.status.to_s })
        else
          Result.failure(:upstream_failed, 'bot_stop_failed', "Failed to stop bot '#{bot.label}'.")
        end
      end
    end
  end
end
