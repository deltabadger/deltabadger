# frozen_string_literal: true

module BotApi
  module Bots
    # Updates label / quote_amount on a stopped bot. The MCP tool only exposes
    # these two fields; REST keeps the same surface for now — adding more knobs
    # is a separate decision, not a free side-effect of the extraction.
    class UpdateSettings
      def self.call(user:, bot_id:, quote_amount: nil, label: nil)
        new(user: user, bot_id: bot_id, quote_amount: quote_amount, label: label).call
      end

      def initialize(user:, bot_id:, quote_amount: nil, label: nil)
        @user = user
        @bot_id = bot_id
        @quote_amount = quote_amount
        @label = label
      end

      def call
        bot = @user.bots.not_deleted.find_by(id: @bot_id.to_i)
        return Result.failure(:not_found, 'bot_not_found', 'Bot not found.') unless bot

        if bot.working?
          return Result.failure(:conflict, 'bot_running',
                                "Bot must be stopped before updating settings. Current status: #{bot.status}.")
        end

        updates = {}
        updates[:quote_amount] = @quote_amount if @quote_amount.present?
        updates[:label] = @label if @label.present?

        return Result.failure(:validation_failed, 'no_updates_provided', 'No settings provided to update.') if updates.empty?

        bot.quote_amount = updates[:quote_amount] if updates[:quote_amount]
        bot.label = updates[:label] if updates[:label]
        bot.set_missed_quote_amount

        if bot.save
          Result.success({ id: bot.id, label: bot.label, updated: updates.keys.map(&:to_s) })
        else
          Result.failure(:validation_failed, 'bot_save_failed',
                         "Failed to update bot: #{bot.errors.full_messages.join(', ')}")
        end
      end
    end
  end
end
