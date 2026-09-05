# frozen_string_literal: true

module BotApi
  module Bots
    # Bot::SmartIntervalable's after_initialize writes into `settings` when a bot is loaded, so a
    # freshly-found bot is already dirty and Bot::Accountable raises on the next save unless the
    # missed amount is recomputed first. Bot#stop does this; #delete, #archive's status write and
    # #unarchive do not, so every lifecycle service does it here before touching the record.
    module LifecycleSupport
      private

      def find_bot(bot_id)
        bot = @user.bots.not_deleted.find_by(id: bot_id.to_i)
        bot&.set_missed_quote_amount if bot.respond_to?(:set_missed_quote_amount)
        bot
      end

      def bot_not_found = Result.failure(:not_found, 'bot_not_found', 'Bot not found.')
    end
  end
end
