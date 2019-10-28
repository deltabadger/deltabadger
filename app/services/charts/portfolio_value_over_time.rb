module Charts
  class PortfolioValueOverTime < BaseService
    def call(bot)
      dates = bot.transactions.select('distinct id, created_at')

      dates
        .map(&:created_at)
        .map do |date|
        total_invested = bot.transactions.where('created_at <  ?', date).sum(&:price)
        value = rand(30)
        [date, total_invested, value]
      end
    end
  end
end
