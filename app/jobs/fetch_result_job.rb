class FetchResultJob < ApplicationJob
  queue_as :default

  # Don't retry - order fetching should be immediate
  discard_on StandardError

  def perform(bot_id, result_parameters, fixing_price)
    FetchOrderResult.call(bot_id, result_parameters, fixing_price)
  end
end
