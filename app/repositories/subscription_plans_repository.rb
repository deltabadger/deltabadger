class SubscriptionPlansRepository < BaseRepository
  def model
    SubscriptionPlan
  end

  def saver
    find_by_name!('saver')
  end

  def investor
    find_by_name!('investor')
  end

  def hodler
    find_by_name!('hodler')
  end

  private

  def find_by_name!(name)
    SubscriptionPlan.find_by!(name: name)
  end
end
