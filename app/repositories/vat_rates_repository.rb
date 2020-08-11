class VatRatesRepository < BaseRepository
  def model
    VatRate
  end

  def all_in_display_order
    alphabetical_order = model.all.order(country: :asc)
    eu = alphabetical_order.select(&:eu?)
    other = alphabetical_order.reject(&:eu?)
    other + eu
  end
end
