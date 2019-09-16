module Api
  class TransactionsController < Api::BaseController
    def csv
      bot = BotsRepository.new.by_id_for_user(user, params[:id])
      file = GenerateTransactionsCsv.call(bot)
      send_file file, filename: file.name
    end
  end
end
