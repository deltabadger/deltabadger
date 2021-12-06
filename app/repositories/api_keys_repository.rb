class ApiKeysRepository < BaseRepository
  def model
    ApiKey
  end

  def for_bot(user_id, exchange_id, key_type = 'trading')
    model.where(user_id: user_id, exchange_id: exchange_id, key_type: key_type).first
  end
end
