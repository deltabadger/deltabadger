# frozen_string_literal: true

module BotApi
  module Bots
    class Start
      def self.call(user:, bot_id:, start_fresh: nil)
        new(user: user, bot_id: bot_id, start_fresh: start_fresh).call
      end

      # The web Start's flag (Bots::StartsController), read the one way for every path: absent starts
      # fresh, as the plain Start button does; true/false (or 1/0) is the restart modal's Skip or
      # Continue; anything else is nil, refused rather than guessed.
      def self.start_fresh(value)
        value.nil? || Utilities::String.to_boolean(value)
      rescue ArgumentError
        nil
      end

      # The wash-sale question is owed at the first thing that can sell, and every start of such a bot
      # (web, MCP, REST) waits for the answer. A bot that only buys is never held up by it.
      def self.wash_sale_question_owed?(user, bot) = !user.wash_sale_decided? && bot.sell_capable?

      def initialize(user:, bot_id:, start_fresh: nil)
        @user = user
        @bot_id = bot_id
        @start_fresh = self.class.start_fresh(start_fresh)
      end

      def call
        bot = @user.bots.not_deleted.find_by(id: @bot_id.to_i)
        return Result.failure(:not_found, 'bot_not_found', 'Bot not found.') unless bot
        if self.class.wash_sale_question_owed?(@user, bot)
          return Result.failure(:validation_failed, 'wash_sale_question_owed', I18n.t('engine.wash_sale_question_owed', label: bot.label))
        end
        return Result.failure(:validation_failed, 'invalid_start_fresh', I18n.t('engine.invalid_start_fresh')) if @start_fresh.nil?

        if bot.working?
          return Result.failure(:conflict, 'bot_already_running',
                                "Bot '#{bot.label}' is already running (#{bot.status}).",
                                data: { id: bot.id, label: bot.label, status: bot.status.to_s })
        end

        # Straight to Bot#start, as the web start does: no carry captured here that the web would not capture.
        if bot.start(start_fresh: @start_fresh)
          Result.success({ id: bot.id, label: bot.label, status: bot.status.to_s })
        else
          Result.failure(:validation_failed, 'bot_start_failed',
                         "Failed to start bot '#{bot.label}': #{bot.errors.full_messages.join(', ')}")
        end
      end
    end
  end
end
