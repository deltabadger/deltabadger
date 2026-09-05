# frozen_string_literal: true

module BotApi
  module Tracker
    # A price the user states for one row, in USD — the ledger's unit. Everything that reads a
    # priced row goes through PriceService#enrich, which prefers a stated price, so the tiles, the
    # chart, the positions and the tax report all inherit it.
    class SetTransactionPrice
      def self.call(user:, transaction_id:, price_usd:)
        transaction = AccountTransaction.for_user(user).find_by(id: transaction_id.to_i)
        return Result.failure(:not_found, 'transaction_not_found', 'Transaction not found.') unless transaction

        if transaction.venue_valued? && price_usd.present?
          return Result.failure(:conflict, 'venue_valued', 'The venue priced this row; it cannot be restated.')
        end

        # Strict first: parse_manual answers nil for a negative and for garbage alike, and
        # set_manual reads nil as "clear", so without this '-5' would delete the stated price.
        if price_usd.present? && Number.parse(price_usd).nil?
          return Result.failure(:validation_failed, 'invalid_price',
                                'price_usd must be a non-negative number, or empty to clear.')
        end

        transaction.set_manual(:price, price_usd.presence)
        transaction.save!
        ::Tracker::LedgerJob.perform_later(user.id)
        Result.success({ id: transaction.id, price_usd: transaction.manual_value(:price)&.to_s('F') })
      end
    end
  end
end
