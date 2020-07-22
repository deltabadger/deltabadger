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

  def plan_cache
    @plan_cache ||= SubscriptionPlan.all.map { |sp| [sp.name, sp] }.to_h
  end

  def find_by_name!(name)
    plan_cache.fetch(name)
  end
end
