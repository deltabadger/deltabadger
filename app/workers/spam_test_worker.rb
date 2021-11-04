class SpamTestWorker
  include Sidekiq::Worker

  def perform
    puts TransactionsRepository.new.total_btc_bought
    ProfitableBotsRepository.new.profitable_bots_data(Time.now)
    SpamTestWorker.perform_at(Time.now + 5.second)
  rescue StandardError => e # prevent job from retrying
    Raven.capture_exception(e)
  end
end
