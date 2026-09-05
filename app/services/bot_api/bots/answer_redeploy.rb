# frozen_string_literal: true

module BotApi
  module Bots
    # The "Redeploy $X?" prompt's two answers. Yes puts idle liquidation proceeds back into the
    # composition; No takes that money off the table for good. Both are queued — the jobs hold
    # the exchange semaphore, which is what keeps a decline from interleaving with a batch in
    # flight.
    class AnswerRedeploy
      def self.call(user:, bot_id:, accept:, dry_run: false)
        new(user: user, bot_id: bot_id, accept: accept, dry_run: dry_run).call
      end

      def initialize(user:, bot_id:, accept:, dry_run: false)
        @user = user
        @bot_id = bot_id
        # Strict: a cast that turned nil into false would make "accept omitted" a decline, and
        # declining takes money off the table for good.
        @accept = Boolean.parse(accept)
        @dry_run = dry_run
      end

      def call
        return Result.failure(:validation_failed, 'accept_required', 'accept must be true or false.') if @accept.nil?

        bot = @user.bots.not_deleted.find_by(id: @bot_id.to_i)
        return Result.failure(:not_found, 'bot_not_found', 'Bot not found.') unless bot

        unless bot.respond_to?(:redeploy!)
          return Result.failure(:validation_failed, 'not_composition_bot',
                                'Only index and basket bots have a redeploy offer.')
        end
        return Result.failure(:conflict, 'bot_archived', "Bot '#{bot.label}' is archived; reactivate it first.") if bot.archived?
        return Result.failure(:conflict, 'market_closed', 'The market is closed; try again when it opens.') if @accept && market_closed?(bot)

        queue(bot) unless @dry_run
        Result.success({ id: bot.id, label: bot.label, accepted: @accept, offer: safe_offer(bot), dry_run: @dry_run },
                       status: :accepted)
      end

      private

      def queue(bot)
        if @accept
          Bot::RedeployJob.perform_later(bot)
          bot.log_activity('redeploy_requested', level: :info, details: { user_id: @user.id })
        else
          # The decline job writes its own redeploy_declined activity row.
          Bot::DeclineRedeployJob.perform_later(bot, user_id: @user.id)
        end
      end

      # Reported, never gated on: the web button queues regardless and the jobs handle an empty
      # offer themselves, so refusing here would make the API stricter than the page for no gain.
      # And it reads metrics, which may be cold — a decline must still queue when the read fails.
      def safe_offer(bot)
        bot.redeploy_offer.to_s
      rescue StandardError => e
        Rails.logger.warn("redeploy offer read failed bot=#{bot.id}: #{e.message}")
        nil
      end

      def market_closed?(bot)
        bot.ensure_exchange_authenticated
        !bot.exchange.market_open?(tickers: bot.composition_tickers)
      rescue StandardError => e
        Rails.logger.warn("redeploy market check failed bot=#{bot.id}: #{e.message}")
        false
      end
    end
  end
end
