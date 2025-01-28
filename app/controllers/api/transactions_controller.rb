module Api
  class TransactionsController < ApplicationController
    def csv
      bot = Bot.find(params[:bot_id])
      file = GenerateTransactionsCsv.call(bot)
      filename =
        "bot-#{bot.id}-transactions-#{Time.now.strftime('%F')}.csv"

      send_data(file, filename: filename)
    end
  end
end
