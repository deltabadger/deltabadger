class BotsRepository < BaseRepository
  def by_id_for_user(user, id)
    user.bots.find(id)
  end

  def for_user(user)
    user
      .bots
      .includes(:exchange, :transactions)
      .all
  end

  def model
    Bot
  end
end
