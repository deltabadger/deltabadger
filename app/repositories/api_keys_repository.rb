class ApiKeysRepository < BaseRepository
  def model
    ApiKey
  end

  def for_bot(user_id, exchange_id)
    model.where(user_id: user_id, exchange_id: exchange_id).first
  end
end
