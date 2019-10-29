class ExchangesRepository < BaseRepository
  def model
    Exchange
  end

  def all
    model.order(:name).all
  end
end
