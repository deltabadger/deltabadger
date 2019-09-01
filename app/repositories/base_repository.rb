class BaseRepository
  def save(object)
    object.save!
  end

  def find(id)
    model.find(id)
  end

  def update(id, params)
    model.update(id, params)
  end
end
