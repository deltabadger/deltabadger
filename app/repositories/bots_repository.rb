class BotsRepository < BaseRepository
  def by_id_for_user(user, id)
    user.bots.without_deleted.find(id)
  end

  def for_user(user)
    user
      .bots
      .without_deleted
      .includes(:exchange, :transactions)
      .all
  end

  def count_with_status(status)
    model
      .where(status: status)
      .count
  end

  def list_top_ten
    most_popular_bots(10)
  end

  def model
    Bot
  end

  private

  def most_popular_bots(amount)
    model.group("bots.settings->>'base'").where(status: "working").order(count: :desc).limit(amount).count
  end

end
