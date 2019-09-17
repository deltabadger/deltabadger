module Api
  class TransactionsController < ApplicationController
    def csv
      bot = BotsRepository.new.by_id_for_user(current_user, params[:bot_id])
      file = GenerateTransactionsCsv.call(bot)
      filename =
        "bot-#{bot.id}-transactions-#{Time.now.strftime('%F')}.csv"

      send_data(file, filename: filename)
    end
  end
end
