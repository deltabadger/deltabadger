# frozen_string_literal: true

module BotApi
  module Transactions
    # Lists the user's bot trades. `bot_id` optionally narrows to one bot.
    # `limit` is clamped to [1, 100] with a default of 20 (matches MCP).
    class List
      DEFAULT_LIMIT = 20
      MAX_LIMIT = 100

      def self.call(user:, bot_id: nil, limit: nil)
        new(user: user, bot_id: bot_id, limit: limit).call
      end

      def initialize(user:, bot_id: nil, limit: nil)
        @user = user
        @bot_id = bot_id
        @limit = limit
      end

      def call
        if @bot_id.present?
          bot = @user.bots.not_deleted.find_by(id: @bot_id.to_i)
          return Result.failure(:not_found, 'bot_not_found', 'Bot not found.') unless bot
        end

        capped_limit = clamp_limit(@limit)
        scope = @user.transactions.order(created_at: :desc)
        scope = scope.where(bot_id: bot.id) if bot
        rows = scope.limit(capped_limit).map { |txn| row_for(txn) }

        Result.success({ count: rows.size, transactions: rows })
      end

      private

      def clamp_limit(raw)
        value = raw&.to_i || DEFAULT_LIMIT
        return DEFAULT_LIMIT if value <= 0

        [value, MAX_LIMIT].min
      end

      def row_for(txn)
        {
          id: txn.id,
          bot_id: txn.bot_id,
          created_at: txn.created_at,
          side: txn.side.to_s,
          status: txn.status.to_s,
          amount_exec: txn.amount_exec,
          base: txn.base,
          price: txn.price,
          quote: txn.quote,
          quote_amount_exec: txn.quote_amount_exec
        }
      end
    end
  end
end
