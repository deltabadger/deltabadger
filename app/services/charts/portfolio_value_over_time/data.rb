module Charts::PortfolioValueOverTime
  class Data < BaseService
    def call(bot)
      transactions = bot.transactions.submitted.order(created_at: :asc)
      return [] if transactions.empty?

      total_invested = 0
      total_accumulated = 0

      transactions.map do |t|
        next unless t.price.present? && t.amount.present?

        total_invested += t.price * t.amount
        total_accumulated += t.amount
        current_value = t.price * total_accumulated

        [t.created_at, total_invested, current_value]
      end.compact
    end
  end
end
