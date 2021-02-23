class BotsRepository < BaseRepository
  def by_id_for_user(user, id)
    user.bots.find(id)
  end

  def for_user(user)
    user
      .bots
      .where.not(status: 'deleted')
      .includes(:exchange, :transactions)
      .all
  end

  def count_with_status(status)
    model
      .where(status: status)
      .count
  end

  def model
    Bot
  end
end
