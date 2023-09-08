class ExchangesRepository < BaseRepository
  def model
    Exchange
  end

  def all
    model.order(:name).where.not(name: ["FTX", "FTX.US"])
  end
end
