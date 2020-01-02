class PaymentsRepository < BaseRepository
  def paid
    model.where(status: :paid)
  end

  def model
    Payment
  end
end
